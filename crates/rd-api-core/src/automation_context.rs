//! Turning a bus event into the triggers it satisfies and the context conditions see.
//!
//! The event bus carries more than automations should hang off — configuration changes,
//! captcha prompts, tracker statistics — so this is a closed translation rather than a pass
//! through. One event may satisfy several triggers: a download that leaves `resolving` for
//! `downloading` has both resolved and started, and saying so is more honest than picking
//! one and leaving the other trigger permanently dead.

use rd_automation::{EventContext, Field, Trigger};
use rd_core::{EventEnvelope, EventKind, PackageId};
use rd_notify::NotificationEvent;

/// What an event means to the automation engine.
pub(crate) struct EventMatch {
    pub triggers: Vec<Trigger>,
    pub context: EventContext,
    /// The package the actions operate on, when the event names one.
    pub package_id: Option<PackageId>,
}

/// Whether [`classify`] can find a trigger in an event of this kind at all; asked before
/// anything is read for it.
pub(crate) fn may_trigger(event: &EventEnvelope) -> bool {
    matches!(
        event.kind,
        EventKind::CollectorIntake
            | EventKind::DownloadState
            | EventKind::PackageState
            | EventKind::PostprocessProgress
            | EventKind::StorageCapacity
            | EventKind::SubscriptionChanged
            | EventKind::UsenetChanged
    )
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
        EventKind::StorageCapacity => Ok(storage(event)),
        EventKind::SubscriptionChanged => subscription(database, event).await,
        EventKind::UsenetChanged => usenet_job_hopeless(database, event).await,
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
    // One row, not the table: this runs for every state change of every download (DB-05).
    let Ok(id) = id.parse() else {
        return Ok(None);
    };
    let Some(file) = database.get_download(id).await? else {
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

/// A package that just finished or failed, decided on the event's payload exactly as the
/// notification hub decides it (RD-1270-03, `notify_service::transitions::package_outcome`): a
/// reorder, a rename, a stage or the same outcome written again is no new moment, and an outcome
/// read after the package was restarted or removed is still the outcome its event announced. The
/// row gives only what the payload does not carry -- name, kind, size and category -- and a
/// package removed in the meantime has none of them.
async fn package(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    let trigger = match crate::notify_service::transitions::package_outcome(&event.payload) {
        Some(NotificationEvent::PackageCompleted) => Trigger::PackageCompleted,
        Some(NotificationEvent::PackageFailed) => Trigger::PackageFailed,
        _ => return Ok(None),
    };
    let Some(id) = text(event, "package_id").and_then(|id| id.parse().ok()) else {
        return Ok(None);
    };
    let mut context = EventContext::default();
    context.set(Field::State, text(event, "state").unwrap_or_default());
    if let Some(package) = database.get_package(id).await? {
        context.set(Field::Name, package.name.clone());
        context.set(Field::Kind, format!("{:?}", package.kind).to_lowercase());
        let total: u64 = database
            .downloads_for_package(package.id)
            .await?
            .iter()
            .map(|file| file.total_bytes.map_or(0, rd_core::ByteCount::get))
            .sum();
        if total > 0 {
            context.set_number(Field::SizeBytes, total);
        }
        add_category(database, &mut context, Some(package.id)).await?;
    }
    Ok(Some(EventMatch {
        triggers: vec![trigger],
        context,
        package_id: Some(id),
    }))
}

/// A storage root blocked or released (RD-1270-04). The change is the context, so a condition
/// can tell the two apart; a rule that names none hears the block only (`rd_automation::admits`).
/// An event that names no change is no transition.
fn storage(event: &EventEnvelope) -> Option<EventMatch> {
    let mut context = EventContext::default();
    context.set(Field::Change, text(event, "change")?);
    // The target's key (RD-1270-18): the root id or `fallback`, never its name, so a root named
    // like the download directory cannot pass for it. Name and path stay in the payload.
    if let Some(target) = text(event, "target") {
        context.set(Field::StorageTarget, target);
    }
    Some(EventMatch {
        triggers: vec![Trigger::StorageThreshold],
        context,
        package_id: None,
    })
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

/// A finished poll that accepted at least one new item (RD-1240-21).
///
/// The record of a finished run is the one subscription event that says anything was
/// accepted: every other write announces an anonymous `{"resource":"subscription"}`. One poll
/// is one run however many items it accepted -- a first poll that collects a backlog must not
/// start an automation per entry -- and the poll's own record is written once, so a poll that
/// is repeated over the same feed (and accepts nothing new) fires nothing.
async fn subscription(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    if text(event, "poll") != Some("finished") || text(event, "error").is_some() {
        return Ok(None);
    }
    let accepted = event
        .payload
        .get("accepted")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if accepted == 0 {
        return Ok(None);
    }
    let mut context = EventContext::default();
    context.set(Field::Source, "subscription");
    // The name a condition can match on is the subscription's; a deleted one still fires.
    if let Some(id) = text(event, "subscription_id").and_then(|id| id.parse().ok())
        && let Some(subscription) = database.subscription(id).await?
    {
        context.set(Field::Name, subscription.name);
    }
    Ok(Some(EventMatch {
        triggers: vec![Trigger::SubscriptionItem],
        context,
        package_id: None,
    }))
}

/// A Usenet set given up as beyond repair (RD-1100-02); every other `usenet.changed` is
/// configuration or an import arriving, nothing to act on.
async fn usenet_job_hopeless(
    database: &rd_db::Database,
    event: &EventEnvelope,
) -> anyhow::Result<Option<EventMatch>> {
    if text(event, "state") != Some("hopeless") {
        return Ok(None);
    }
    let mut context = EventContext::default();
    context.set(Field::State, "failed");
    context.set(Field::Kind, "usenet");
    context.set(Field::FailureCode, rd_db::USENET_JOB_HOPELESS);
    let package_id: Option<PackageId> = text(event, "package_id").and_then(|id| id.parse().ok());
    if let Some(id) = package_id
        && let Some(package) = database.get_package(id).await?
    {
        context.set(Field::Name, package.name);
    }
    add_category(database, &mut context, package_id).await?;
    Ok(Some(EventMatch {
        triggers: vec![Trigger::UsenetJobHopeless],
        context,
        package_id,
    }))
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
    let Some(package) = database.get_package(package_id).await? else {
        anyhow::bail!("package not found");
    };
    context.set(Field::Name, package.name.clone());
    context.set(Field::State, package.state.to_string());
    context.set(Field::Kind, format!("{:?}", package.kind).to_lowercase());
    let files = database.downloads_for_package(package_id).await?;
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
        .get_package(package_id)
        .await?
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
