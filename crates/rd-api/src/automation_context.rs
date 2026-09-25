//! Turning a bus event into the triggers it satisfies and the context conditions see.
//!
//! The event bus carries more than automations should hang off — configuration changes,
//! captcha prompts, tracker statistics — so this is a closed translation rather than a pass
//! through. One event may satisfy several triggers: a download that leaves `resolving` for
//! `downloading` has both resolved and started, and saying so is more honest than picking
//! one and leaving the other trigger permanently dead.

use rd_automation::{EventContext, Field, Trigger};
use rd_core::{EventEnvelope, EventKind, PackageId, PackageState};

/// What an event means to the automation engine.
pub(crate) struct EventMatch {
    pub triggers: Vec<Trigger>,
    pub context: EventContext,
    /// The package the actions operate on, when the event names one.
    pub package_id: Option<PackageId>,
}

/// Classifies an event, loading only what the context actually needs.
pub(crate) async fn classify(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    match event.kind {
        EventKind::CollectorIntake => Ok(Some(intake(event))),
        EventKind::DownloadState => download(database, event).await,
        EventKind::PackageState => package(database, event).await,
        EventKind::PostprocessProgress => Ok(postprocess(event)),
        EventKind::StorageCapacity => Ok(Some(EventMatch {
            triggers: vec![Trigger::StorageThreshold],
            context: EventContext::default(),
            package_id: None,
        })),
        EventKind::SubscriptionChanged => Ok(subscription(event)),
        _ => Ok(None),
    }
}

fn text<'a>(event: &'a EventEnvelope, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(serde_json::Value::as_str)
}

fn intake(event: &EventEnvelope) -> EventMatch {
    let mut context = EventContext::default();
    if let Some(source) = text(event, "source") {
        context.set(Field::Source, source);
    }
    EventMatch {
        triggers: vec![Trigger::IntakeReceived],
        context,
        package_id: None,
    }
}

async fn download(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    let Some(id) = text(event, "download_id") else {
        return Ok(None);
    };
    let state = text(event, "state").unwrap_or_default();
    let previous = text(event, "previous").unwrap_or_default();
    let mut triggers = Vec::new();
    // Resolution finishing and the transfer starting are different moments even when one
    // transition covers both, so both triggers fire rather than one shadowing the other.
    if previous == "resolving" {
        triggers.push(Trigger::DownloadResolved);
    }
    match state {
        "downloading" => triggers.push(Trigger::DownloadStarted),
        "completed" => triggers.push(Trigger::DownloadCompleted),
        "failed" => triggers.push(Trigger::DownloadFailed),
        _ => {}
    }
    if triggers.is_empty() {
        return Ok(None);
    }
    let Some(file) = database
        .list_downloads()
        .await?
        .into_iter()
        .find(|file| file.id.to_string() == id)
    else {
        return Ok(None);
    };
    let mut context = EventContext::default();
    context.set(Field::Name, file.file_name.clone());
    context.set(Field::State, state);
    context.set(Field::Kind, format!("{:?}", file.kind).to_lowercase());
    if let Some(host) = file.source.host_str() {
        context.set(Field::Domain, host);
    }
    if let Some(extension) = file.file_name.rsplit_once('.').map(|(_, tail)| tail) {
        context.set(Field::Extension, extension);
    }
    if let Some(total) = file.total_bytes {
        context.set_number(Field::SizeBytes, total.get());
    }
    if let Some(code) = file
        .last_error
        .as_ref()
        .and_then(|failure| failure.code.clone())
    {
        context.set(Field::FailureCode, code);
    }
    add_category(database, &mut context, Some(file.package_id)).await?;
    Ok(Some(EventMatch {
        triggers,
        context,
        package_id: Some(file.package_id),
    }))
}

async fn package(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    let Some(id) = text(event, "package_id") else {
        return Ok(None);
    };
    let Some(package) = database
        .list_packages()
        .await?
        .into_iter()
        .find(|package| package.id.to_string() == id)
    else {
        return Ok(None);
    };
    let trigger = match package.state {
        PackageState::Completed => Trigger::PackageCompleted,
        PackageState::Failed => Trigger::PackageFailed,
        _ => return Ok(None),
    };
    let mut context = EventContext::default();
    context.set(Field::Name, package.name.clone());
    context.set(Field::State, package.state.to_string());
    context.set(Field::Kind, format!("{:?}", package.kind).to_lowercase());
    let total: u64 = database
        .list_downloads()
        .await?
        .iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| file.total_bytes.map_or(0, rd_core::ByteCount::get))
        .sum();
    if total > 0 {
        context.set_number(Field::SizeBytes, total);
    }
    add_category(database, &mut context, Some(package.id)).await?;
    Ok(Some(EventMatch {
        triggers: vec![trigger],
        context,
        package_id: Some(package.id),
    }))
}

fn postprocess(event: &EventEnvelope) -> Option<EventMatch> {
    // Only a finished step is a moment worth acting on; progress ticks are noise here.
    if text(event, "state") != Some("completed") {
        return None;
    }
    let trigger = match text(event, "kind")? {
        "extract_zip" | "extract_seven_zip" | "extract_rar" => Trigger::ExtractionFinished,
        "script" => Trigger::ScriptFinished,
        "upload" => Trigger::UploadFinished,
        _ => return None,
    };
    let mut context = EventContext::default();
    context.set(Field::State, "completed");
    if let Some(path) = text(event, "source_path") {
        context.set(Field::Name, path);
    }
    let package_id = text(event, "owner_id").and_then(|id| id.parse().ok());
    Some(EventMatch {
        triggers: vec![trigger],
        context,
        package_id,
    })
}

fn subscription(event: &EventEnvelope) -> Option<EventMatch> {
    // The subscription bus event covers every change to a subscription; only an accepted
    // item is something to act on.
    event.payload.get("accepted_items")?;
    let mut context = EventContext::default();
    context.set(Field::Source, "subscription");
    if let Some(name) = text(event, "name") {
        context.set(Field::Name, name);
    }
    Some(EventMatch {
        triggers: vec![Trigger::SubscriptionItem],
        context,
        package_id: None,
    })
}

/// The context a dry run is evaluated against, built from a real package.
///
/// The editor's dry run needs something concrete to judge a condition by, and a package the
/// author can point at is the only honest source: an invented event would tell them their
/// rule works against data that never existed.
pub(crate) async fn package_context(
    database: &rd_db::Database,
    package_id: Option<PackageId>,
) -> anyhow::Result<EventContext> {
    let mut context = EventContext::default();
    let Some(package_id) = package_id else {
        return Ok(context);
    };
    let Some(package) = database
        .list_packages()
        .await?
        .into_iter()
        .find(|package| package.id == package_id)
    else {
        anyhow::bail!("package not found");
    };
    context.set(Field::Name, package.name.clone());
    context.set(Field::State, package.state.to_string());
    context.set(Field::Kind, format!("{:?}", package.kind).to_lowercase());
    let files: Vec<_> = database
        .list_downloads()
        .await?
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .collect();
    let total: u64 = files
        .iter()
        .map(|file| file.total_bytes.map_or(0, rd_core::ByteCount::get))
        .sum();
    if total > 0 {
        context.set_number(Field::SizeBytes, total);
    }
    if let Some(file) = files.first() {
        if let Some(host) = file.source.host_str() {
            context.set(Field::Domain, host);
        }
        if let Some((_, extension)) = file.file_name.rsplit_once('.') {
            context.set(Field::Extension, extension);
        }
        if let Some(code) = file
            .last_error
            .as_ref()
            .and_then(|failure| failure.code.clone())
        {
            context.set(Field::FailureCode, code);
        }
    }
    add_category(database, &mut context, Some(package_id)).await?;
    Ok(context)
}

/// Adds the category name of a package, when it has one.
async fn add_category(
    database: &rd_db::Database,
    context: &mut EventContext,
    package_id: Option<PackageId>,
) -> anyhow::Result<()> {
    let Some(package_id) = package_id else {
        return Ok(());
    };
    let Some(category_id) = database
        .list_packages()
        .await?
        .into_iter()
        .find(|package| package.id == package_id)
        .and_then(|package| package.category_id)
    else {
        return Ok(());
    };
    if let Some(category) = database
        .list_categories()
        .await?
        .into_iter()
        .find(|category| category.id == category_id)
    {
        context.set(Field::Category, category.name);
    }
    Ok(())
}
