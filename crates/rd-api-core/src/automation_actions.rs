//! Executing one automation action.
//!
//! Each action reaches exactly one capability and nothing else. There is no "run a command"
//! action and no way to name a path: a script is a file name inside the configured scripts
//! directory, a webhook is one of the configured notification targets, and the queue actions
//! go through the scheduler like every other pause or resume.

use rd_automation::Action;
use rd_core::PackageId;

/// Everything an action may reach.
#[derive(Clone)]
pub(crate) struct ActionContext {
    pub database: rd_db::Database,
    pub secrets: rd_secrets::SecretStore,
    pub scheduler: rd_scheduler::SchedulerHandle,
    pub extraction: rd_extract::ExtractionService,
    pub http: reqwest::Client,
}

/// Runs one action. `Err` is retryable; the caller decides when to give up.
pub(crate) async fn execute(
    context: &ActionContext,
    action: &Action,
    package_id: Option<PackageId>,
    trigger: rd_automation::Trigger,
) -> anyhow::Result<()> {
    match action {
        Action::PausePackage => queue_action(context, package_id, true).await,
        Action::ResumePackage => queue_action(context, package_id, false).await,
        Action::SetCategory { category_id } => {
            set_category(context, package_id, *category_id).await
        }
        Action::Script { name } => script(context, name, package_id, trigger).await,
        Action::Webhook { target_id } => webhook(context, *target_id, package_id, trigger).await,
    }
}

async fn queue_action(
    context: &ActionContext,
    package_id: Option<PackageId>,
    pause: bool,
) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        anyhow::bail!("this trigger names no package");
    };
    for file in context
        .database
        .list_downloads()
        .await?
        .into_iter()
        .filter(|file| file.package_id == package_id)
    {
        let result = if pause {
            context.scheduler.pause(file.id).await
        } else {
            context.scheduler.resume(file.id).await
        };
        // A file that cannot change state — already finished, already paused — is not a
        // failure of the automation; the action is about the package, not each row.
        if let Err(error) = result {
            tracing::debug!(%error, download = %file.id, "automation queue action skipped");
        }
    }
    Ok(())
}

async fn set_category(
    context: &ActionContext,
    package_id: Option<PackageId>,
    category_id: rd_core::CategoryId,
) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        anyhow::bail!("this trigger names no package");
    };
    let Some(package) = context
        .database
        .list_packages()
        .await?
        .into_iter()
        .find(|package| package.id == package_id)
    else {
        anyhow::bail!("package no longer exists");
    };
    anyhow::ensure!(
        context
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id),
        "category no longer exists"
    );
    // The same layout the enqueue path builds: every package keeps its own folder below the
    // category directory, so a moved package is indistinguishable from one created there.
    let root = crate::destination::resolve_destination(&context.database, Some(category_id))
        .await?
        .unwrap_or_else(|| context.scheduler.downloads_directory().to_path_buf());
    let directory = rd_files::package_directory(&root, &package.name);
    context
        .database
        .update_packages(
            vec![package_id],
            rd_db::PackageChange {
                category: Some(rd_db::CategoryAssignment {
                    category_id: Some(category_id),
                    destinations: [(package_id, directory.to_string_lossy().into_owned())]
                        .into_iter()
                        .collect(),
                }),
                priority: None,
                name: None,
                password: None,
                postprocess_level: None,
                script: None,
            },
        )
        .await?;
    Ok(())
}

async fn script(
    context: &ActionContext,
    name: &str,
    package_id: Option<PackageId>,
    trigger: rd_automation::Trigger,
) -> anyhow::Result<()> {
    let mut about = rd_extract::StandaloneScript {
        kind: format!("automation:{}", trigger_slug(trigger)),
        ..rd_extract::StandaloneScript::default()
    };
    if let Some(package_id) = package_id
        && let Some(package) = context
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == package_id)
    {
        about.package_id = package.id.to_string();
        about.package_name = package.name.clone();
        about.final_dir = Some(std::path::PathBuf::from(&package.destination));
    }
    let ok = context.extraction.run_named_script(name, &about).await?;
    anyhow::ensure!(ok, "script reported failure");
    Ok(())
}

async fn webhook(
    context: &ActionContext,
    target_id: rd_core::NotificationTargetId,
    package_id: Option<PackageId>,
    trigger: rd_automation::Trigger,
) -> anyhow::Result<()> {
    let Some(target) = context
        .database
        .list_notification_targets()
        .await?
        .into_iter()
        .find(|target| target.id == target_id)
    else {
        anyhow::bail!("notification target no longer exists");
    };
    anyhow::ensure!(target.enabled, "notification target is disabled");
    // Only the secret this target owns is resolved. An action names a target, never a
    // vault reference, so there is no spelling of an action that reaches another target's
    // credential.
    let secret = match &target.secret_ref {
        Some(reference) => context.secrets.get(reference).await.ok(),
        None => None,
    };
    let name = match package_id {
        Some(package_id) => context
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == package_id)
            .map(|package| package.name),
        None => None,
    };
    let slug = trigger_slug(trigger);
    let message = rd_notify::Message {
        title: format!("Automation: {slug}"),
        body: name.clone().unwrap_or_else(|| "no package".to_owned()),
        event: rd_notify::NotificationEvent::PackageCompleted,
        idempotency_key: String::new(),
        payload: serde_json::json!({
            "trigger": slug,
            "package": name,
            "package_id": package_id.map(|id| id.to_string()),
        }),
    };
    // The same fallback the notification hub uses: a target whose stored config does not
    // parse still delivers with defaults rather than failing every automation that names it.
    let config: rd_notify::TargetConfig =
        serde_json::from_value(target.config.clone()).unwrap_or_default();
    let vendor = crate::notify_service::vendor_directory(&context.database).await;
    let attempt = rd_notify::send(
        &context.http,
        &target,
        &config,
        &message,
        secret.as_ref(),
        vendor.as_deref(),
    )
    .await;
    anyhow::ensure!(
        attempt.ok,
        "{}",
        attempt
            .excerpt
            .unwrap_or_else(|| "webhook was not delivered".to_owned())
    );
    Ok(())
}

/// Stable slug of a trigger, for script environments and webhook payloads.
fn trigger_slug(trigger: rd_automation::Trigger) -> String {
    serde_json::to_string(&trigger)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}
