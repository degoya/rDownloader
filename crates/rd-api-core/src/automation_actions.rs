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
    /// The LinkGrabber's intake, for `add_links`; absent only in a test that dispatches
    /// nothing, where the action then fails as a retryable error.
    pub links: Option<crate::automation_links::LinkIntake>,
}

/// Runs one action. `Err` is retryable; the caller decides when to give up.
///
/// `delivery_key` names this action of this run -- the run's id and the action's index -- and
/// is the idempotency key a webhook or notify action sends, the same on every retry.
pub(crate) async fn execute(
    context: &ActionContext,
    action: &Action,
    package_id: Option<PackageId>,
    trigger: rd_automation::Trigger,
    delivery_key: &str,
) -> anyhow::Result<()> {
    match action {
        Action::PausePackage => queue_action(context, package_id, true).await,
        Action::ResumePackage => queue_action(context, package_id, false).await,
        Action::SetCategory { category_id } => {
            set_category(context, package_id, *category_id).await
        }
        Action::Script { name } => script(context, name, package_id, trigger).await,
        Action::Webhook { target_id } => {
            let about = About {
                package_id,
                trigger,
                delivery_key,
            };
            webhook(context, *target_id, &about, None).await
        }
        Action::SetPriority { priority } => set_priority(context, package_id, *priority).await,
        Action::PauseQueue => pause_queue(context).await,
        Action::StartQueue => start_queue(context).await,
        Action::ExtractPackage => extract(context, package_id).await,
        Action::Notify { target_id, message } => {
            let about = About {
                package_id,
                trigger,
                delivery_key,
            };
            webhook(context, *target_id, &about, Some(message.as_str())).await
        }
        Action::AddLinks { links, destination } => {
            crate::automation_links::add_links(context, links, *destination).await
        }
    }
}

async fn set_priority(
    context: &ActionContext,
    package_id: Option<PackageId>,
    priority: rd_core::DownloadPriority,
) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        anyhow::bail!("this trigger names no package");
    };
    anyhow::ensure!(
        context.database.get_package(package_id).await?.is_some(),
        "package no longer exists"
    );
    context
        .database
        .update_packages(
            vec![package_id],
            rd_db::PackageChange {
                category: None,
                priority: Some(priority),
                name: None,
                password: None,
                postprocess_level: None,
                script: None,
            },
        )
        .await?;
    Ok(())
}

/// Pauses the whole queue as "pause all" does, until a `start_queue` action or the queue's own
/// resume ends it (RD-1240-30). A pause already in force keeps the files it holds and loses its
/// end, so it now lasts until the queue is started.
async fn pause_queue(context: &ActionContext) -> anyhow::Result<()> {
    let pause = context.scheduler.pause_queue_until_started().await?;
    tracing::info!(files = pause.files.len(), "an automation paused the queue");
    Ok(())
}

/// Ends a pause of the whole queue, as the queue's own resume does. A queue that is not paused
/// is already started, which is what the author asked for, so that is no failure.
async fn start_queue(context: &ActionContext) -> anyhow::Result<()> {
    if context.scheduler.queue_pause().await.is_some() {
        let resumed = context.scheduler.resume_queue().await?;
        tracing::info!(resumed, "an automation started the queue");
    }
    Ok(())
}

/// Unpacks the package's completed files, as the package menu's "Extract" does: the same
/// request, and the same refusal when nothing in the package has finished yet.
async fn extract(context: &ActionContext, package_id: Option<PackageId>) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        anyhow::bail!("this trigger names no package");
    };
    let completed = context
        .database
        .downloads_for_package(package_id)
        .await?
        .iter()
        .any(|file| file.state == rd_core::DownloadState::Completed);
    anyhow::ensure!(completed, "the package has no completed files to extract");
    context
        .extraction
        .request(package_id, rd_extract::ExtractionTrigger::Manual)
        .await
}

async fn queue_action(
    context: &ActionContext,
    package_id: Option<PackageId>,
    pause: bool,
) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        anyhow::bail!("this trigger names no package");
    };
    for file in context.database.downloads_for_package(package_id).await? {
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
    let Some(package) = context.database.get_package(package_id).await? else {
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
        && let Some(package) = context.database.get_package(package_id).await?
    {
        about.package_id = package.id.to_string();
        about.package_name = package.name.clone();
        about.final_dir = Some(std::path::PathBuf::from(&package.destination));
    }
    let ok = context.extraction.run_named_script(name, &about).await?;
    anyhow::ensure!(ok, "script reported failure");
    Ok(())
}

/// What a webhook or notify action is about: its package, its trigger and its delivery key.
struct About<'a> {
    package_id: Option<PackageId>,
    trigger: rd_automation::Trigger,
    delivery_key: &'a str,
}

async fn webhook(
    context: &ActionContext,
    target_id: rd_core::NotificationTargetId,
    about: &About<'_>,
    // The author's own text, for a `notify` action; a webhook sends the event alone.
    text: Option<&str>,
) -> anyhow::Result<()> {
    let package_id = about.package_id;
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
        // One the vault holds but cannot open fails the run with its code rather than sending
        // without it (RD-1240-36).
        Some(reference) => match context.secrets.get(reference).await {
            Ok(secret) => Some(secret),
            Err(error) if rd_secrets::is_unreadable(&error) => return Err(error),
            Err(_) => None,
        },
        None => None,
    };
    let name = match package_id {
        Some(package_id) => context
            .database
            .get_package(package_id)
            .await?
            .map(|package| package.name),
        None => None,
    };
    let message = automation_message(about, text, name.as_deref());
    // The same fallback the notification hub uses: a target whose stored config does not
    // parse still delivers with defaults rather than failing every automation that names it.
    let config: rd_notify::TargetConfig =
        serde_json::from_value(target.config.clone()).unwrap_or_default();
    let vendor = crate::notify_service::vendor_directory(&context.database).await;
    let attempt = rd_notify::send(
        &crate::notify_service::webhook_reach(&target),
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

/// The message a webhook or notify action sends (RD-1240-28): labelled as an automation's,
/// not as a finished package, and keyed by its run and action, so a receiver can tell a retry
/// from a second message -- the key had been empty and the label `package_completed`.
fn automation_message(
    about: &About<'_>,
    text: Option<&str>,
    name: Option<&str>,
) -> rd_notify::Message {
    let slug = trigger_slug(about.trigger);
    let mut payload = serde_json::json!({
        "event": rd_notify::NotificationEvent::Automation,
        "trigger": slug,
        "package": name,
        "package_id": about.package_id.map(|id| id.to_string()),
    });
    if let Some(text) = text {
        payload["message"] = serde_json::Value::from(text.trim());
    }
    rd_notify::Message {
        title: format!("Automation: {slug}"),
        body: message_body(text, name),
        event: rd_notify::NotificationEvent::Automation,
        idempotency_key: about.delivery_key.to_owned(),
        payload,
    }
}

/// The text a webhook or notify action sends: the author's message when there is one, then
/// the package's name; a webhook without a package says so.
fn message_body(text: Option<&str>, package: Option<&str>) -> String {
    match (text.map(str::trim), package) {
        (Some(text), Some(package)) => format!("{text}\n{package}"),
        (Some(text), None) => text.to_owned(),
        (None, package) => package.unwrap_or("no package").to_owned(),
    }
}

/// Stable slug of a trigger, for script environments and webhook payloads.
fn trigger_slug(trigger: rd_automation::Trigger) -> String {
    serde_json::to_string(&trigger)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

#[cfg(test)]
#[path = "automation_actions_tests.rs"]
mod tests;
