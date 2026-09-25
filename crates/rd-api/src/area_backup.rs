//! Export and import of one configuration area at a time: subscriptions, streams, automations.
//!
//! Modelled on `routing_backup` rather than on `settings_backup`. The distinction is what the
//! file is for: a settings bundle is a backup of one instance, replaces everything and carries
//! encrypted secrets behind a passphrase. This is a way to hand a few subscriptions to another
//! installation, so it merges by name, refuses nothing it can skip, and carries no secrets at
//! all — which is also why it needs no passphrase.
//!
//! One format with a section per area, and one route pair per area. An import applies only its
//! own section and says so when the file has none, because importing a stream file on the
//! subscriptions page should tell you rather than quietly do nothing.
//!
//! Everything crossing an instance boundary is referenced by name: a category, a notification
//! target, a stream channel. Ids are meaningless on the other side.

use std::collections::{HashMap, HashSet};

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

const BUNDLE_FORMAT: &str = "rdownloader-area-bundle";
const BUNDLE_VERSION: u32 = 1;

/// The most entries one section of a bundle may carry.
///
/// Every entry costs one trip through the serialized database writer, and the request body limit
/// allows 65 MiB of them — enough for tens of thousands. Importing such a bundle holds the single
/// writer for minutes, and every running download's progress write queues up behind it. The cap is
/// checked before the first write, so an oversized bundle is refused rather than half-applied; 500
/// is far beyond any bundle this application exports.
const MAX_SECTION_ENTRIES: usize = 500;

/// Refuses a section long enough to monopolise the writer.
fn check_section(len: usize) -> Result<(), ApiError> {
    if len > MAX_SECTION_ENTRIES {
        return Err(crate::error_codes::bulk_range(MAX_SECTION_ENTRIES));
    }
    Ok(())
}

/// One subscription, with its destination category named rather than referenced.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaSubscription {
    pub name: String,
    pub url: String,
    pub kind: rd_core::SubscriptionKind,
    pub enabled: bool,
    pub mode: rd_core::SubscriptionMode,
    #[serde(default)]
    pub category_name: Option<String>,
    #[serde(default)]
    pub priority: rd_core::DownloadPriority,
    pub interval_seconds: u32,
    #[serde(default)]
    pub filters: rd_core::SubscriptionFilters,
    #[serde(default)]
    pub backlog: rd_core::BacklogPolicy,
    #[serde(default)]
    pub category_map: Vec<rd_core::CategoryMapping>,
    #[serde(default)]
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21). Absent from a
    /// file written before it existed, which imports as the default it always had.
    #[serde(default)]
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits (RD-120-37). Absent from a file written
    /// before it existed, which reads as the list every subscription showed then.
    #[serde(default)]
    pub view: rd_core::SubscriptionView,
    /// Whether the card slider turns its pages on its own (RD-120-37); off when absent.
    #[serde(default)]
    pub autoplay: bool,
    /// The shape of a card's image area (RD-120-42); `2:1` when absent, as it always was.
    #[serde(default)]
    pub card_ratio: rd_core::SubscriptionCardRatio,
    /// Whether the original had an API key. The key itself never travels — it lives in the
    /// vault, and a bundle is a file somebody sends. An import that needs one arrives switched
    /// off, so it cannot poll with no credential and report a failure nobody caused.
    #[serde(default)]
    pub api_key_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaStreamChannel {
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub category_name: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub recording: rd_core::RecordingPolicy,
}

/// One recording schedule; its channel is named, since ids do not survive the trip.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaStreamSchedule {
    pub channel_name: String,
    pub name: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: rd_core::ScheduleKind,
    pub timezone: String,
    pub window_minutes: u32,
    #[serde(default)]
    pub lead_minutes: u32,
    #[serde(default)]
    pub trail_minutes: u32,
    #[serde(default)]
    pub replay_from_start: bool,
}

/// An automation action with every reference resolved to a name.
///
/// Mirrors `rd_automation::Action` rather than reusing it: a webhook points at a notification
/// target by id and a category move at a category by id, and neither id means anything on
/// another instance. `Script` already carries a name, and the two package actions carry nothing.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BundleAreaAction {
    Webhook { target_name: String },
    Script { name: String },
    SetCategory { category_name: String },
    PausePackage,
    ResumePackage,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaAutomation {
    pub name: String,
    pub enabled: bool,
    pub trigger: rd_automation::Trigger,
    #[serde(default)]
    pub condition: rd_automation::ConditionNode,
    pub actions: Vec<BundleAreaAction>,
}

/// One format, a section per area. A file written by one area's export carries only its own.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AreaBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<BundleAreaSubscription>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_channels: Option<Vec<BundleAreaStreamChannel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_schedules: Option<Vec<BundleAreaStreamSchedule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automations: Option<Vec<BundleAreaAutomation>>,
}

impl AreaBundle {
    fn empty() -> Self {
        Self {
            format: BUNDLE_FORMAT.to_owned(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            subscriptions: None,
            stream_channels: None,
            stream_schedules: None,
            automations: None,
        }
    }
}

/// What an import did, per area. Skipped covers both "already there" and "could not be
/// resolved"; the counts are what tells somebody the file was not what they expected.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct ImportAreaSummary {
    pub created: u32,
    pub skipped: u32,
}

fn validate_header(bundle: &AreaBundle) -> Result<(), ApiError> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(ApiError::bad_request(
            "backup.format_unsupported",
            "This file is not an rDownloader area bundle",
        ));
    }
    if bundle.version > BUNDLE_VERSION {
        return Err(ApiError::bad_request(
            "backup.version_unsupported",
            "This bundle was written by a newer version",
        )
        .with_param("version", bundle.version));
    }
    Ok(())
}

/// Refused rather than treated as an empty import: a stream file dropped on the subscriptions
/// page would otherwise report "0 created, 0 skipped" and look like it worked.
fn section_missing(area: &str) -> ApiError {
    ApiError::bad_request(
        "backup.area_missing",
        "This bundle carries no entries for this area",
    )
    .with_param("area", area)
}

// -- subscriptions ----------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/subscriptions/export", tag = "subscriptions", responses((status = 200, body = AreaBundle)))]
pub async fn export_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let name_of = |id: rd_core::CategoryId| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    };
    let entries = state
        .database
        .list_subscriptions()
        .await?
        .into_iter()
        // A script subscription stays behind (RD-130-19): a bundle is a file somebody sends,
        // and what it would carry is an instruction to run code on the machine that opens it.
        .filter(|subscription| subscription.kind != rd_core::SubscriptionKind::Script)
        .map(|subscription| BundleAreaSubscription {
            name: subscription.name,
            url: subscription.url.to_string(),
            kind: subscription.kind,
            enabled: subscription.enabled,
            mode: subscription.mode,
            category_name: subscription.category_id.and_then(name_of),
            priority: subscription.priority,
            interval_seconds: subscription.interval_seconds,
            filters: subscription.filters,
            backlog: subscription.backlog,
            category_map: subscription.category_map,
            source_categories: subscription.source_categories,
            every_release: subscription.every_release,
            view: subscription.view,
            autoplay: subscription.autoplay,
            card_ratio: subscription.card_ratio,
            api_key_required: subscription.secret_ref.is_some(),
        })
        .collect();
    Ok(Json(AreaBundle {
        subscriptions: Some(entries),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/subscriptions/import", tag = "subscriptions", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_subscriptions(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    let entries = bundle
        .subscriptions
        .ok_or_else(|| section_missing("subscriptions"))?;
    check_section(entries.len())?;
    let categories = state.database.list_categories().await?;
    // A set, not a list: the scan runs once per entry, so a linear one made the whole import
    // quadratic in the number of names already stored.
    let mut existing: HashSet<String> = state
        .database
        .list_subscriptions()
        .await?
        .into_iter()
        .map(|subscription| subscription.name)
        .collect();
    let mut summary = ImportAreaSummary::default();
    for entry in entries {
        // Never created from a file, whatever it says (RD-130-19): only the administrator
        // sets up a script subscription, by hand, knowing which script it runs.
        if existing.contains(&entry.name) || entry.kind == rd_core::SubscriptionKind::Script {
            summary.skipped += 1;
            continue;
        }
        let category_id = match &entry.category_name {
            // A category that does not exist here is not a reason to drop the subscription:
            // without one it falls back to the default, which is what a fresh one does anyway.
            Some(name) => categories
                .iter()
                .find(|category| &category.name == name)
                .map(|category| category.id),
            None => None,
        };
        let request = crate::subscription_handlers::SubscriptionRequest {
            name: entry.name.clone(),
            url: entry.url,
            kind: entry.kind,
            // Switched off when the original needed a key, because this bundle has none: a
            // subscription that polls without its credential only produces failures.
            enabled: entry.enabled && !entry.api_key_required,
            mode: entry.mode,
            category_id,
            priority: entry.priority,
            interval_seconds: entry.interval_seconds,
            filters: entry.filters,
            backlog: entry.backlog,
            category_map: entry.category_map,
            source_categories: entry.source_categories,
            every_release: entry.every_release,
            view: entry.view,
            autoplay: entry.autoplay,
            card_ratio: entry.card_ratio.as_str().to_owned(),
            schedule: None,
            api_key: None,
        };
        let Ok(input) = crate::subscription_handlers::subscription_input(&request, None) else {
            summary.skipped += 1;
            continue;
        };
        state.database.create_subscription(input).await?;
        existing.insert(entry.name);
        summary.created += 1;
    }
    Ok(Json(summary))
}

// -- streams ----------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/streams/export", tag = "streams", responses((status = 200, body = AreaBundle)))]
pub async fn export_streams(State(state): State<AppState>) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let channels = state.database.list_stream_channels().await?;
    let schedules = state.database.list_stream_schedules().await?;
    let category_name = |id: rd_core::CategoryId| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    };
    let channel_name = |id: rd_core::StreamChannelId| {
        channels
            .iter()
            .find(|channel| channel.id == id)
            .map(|channel| channel.name.clone())
    };
    let bundled_schedules = schedules
        .iter()
        .filter_map(|schedule| {
            Some(BundleAreaStreamSchedule {
                channel_name: channel_name(schedule.channel_id)?,
                name: schedule.name.clone(),
                enabled: schedule.enabled,
                kind: schedule.kind.clone(),
                timezone: schedule.timezone.clone(),
                window_minutes: schedule.window_minutes,
                lead_minutes: schedule.lead_minutes,
                trail_minutes: schedule.trail_minutes,
                replay_from_start: schedule.replay_from_start,
            })
        })
        .collect();
    let bundled_channels = channels
        .into_iter()
        .map(|channel| BundleAreaStreamChannel {
            url: channel.url,
            name: channel.name,
            quality: channel.quality,
            category_name: channel.category_id.and_then(category_name),
            enabled: channel.enabled,
            recording: channel.recording,
        })
        .collect();
    Ok(Json(AreaBundle {
        stream_channels: Some(bundled_channels),
        stream_schedules: Some(bundled_schedules),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/streams/import", tag = "streams", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_streams(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    if bundle.stream_channels.is_none() && bundle.stream_schedules.is_none() {
        return Err(section_missing("streams"));
    }
    let channel_entries = bundle.stream_channels.unwrap_or_default();
    let schedule_entries = bundle.stream_schedules.unwrap_or_default();
    check_section(channel_entries.len())?;
    check_section(schedule_entries.len())?;
    let categories = state.database.list_categories().await?;
    // Keyed by name because both uses are lookups by name: the duplicate check here, and the
    // channel a schedule attaches to below. As a list both were linear scans inside a loop.
    let mut channels: HashMap<String, rd_core::StreamChannel> = state
        .database
        .list_stream_channels()
        .await?
        .into_iter()
        .map(|channel| (channel.name.clone(), channel))
        .collect();
    let mut summary = ImportAreaSummary::default();

    for entry in channel_entries {
        if channels.contains_key(&entry.name) {
            summary.skipped += 1;
            continue;
        }
        let category_id = entry.category_name.as_ref().and_then(|name| {
            categories
                .iter()
                .find(|category| &category.name == name)
                .map(|category| category.id)
        });
        let request = crate::dto::StreamChannelRequest {
            url: entry.url,
            name: Some(entry.name),
            quality: entry.quality,
            category_id,
            enabled: entry.enabled,
            recording: entry.recording,
        };
        let Ok(input) = crate::stream_handlers::channel_input(request) else {
            summary.skipped += 1;
            continue;
        };
        let created = state.database.create_stream_channel(input).await?;
        channels.insert(created.name.clone(), created);
        summary.created += 1;
    }

    // After the channels, so a schedule can attach to one this same import just created.
    let existing_schedules: HashSet<(String, rd_core::StreamChannelId)> = state
        .database
        .list_stream_schedules()
        .await?
        .into_iter()
        .map(|schedule| (schedule.name, schedule.channel_id))
        .collect();
    for entry in schedule_entries {
        let Some(channel) = channels.get(&entry.channel_name) else {
            summary.skipped += 1;
            continue;
        };
        if existing_schedules.contains(&(entry.name.clone(), channel.id)) {
            summary.skipped += 1;
            continue;
        }
        let request = crate::stream_schedule_handlers::StreamScheduleRequest {
            channel_id: channel.id,
            name: entry.name,
            enabled: entry.enabled,
            kind: entry.kind,
            timezone: entry.timezone,
            window_minutes: entry.window_minutes,
            lead_minutes: entry.lead_minutes,
            trail_minutes: entry.trail_minutes,
            replay_from_start: entry.replay_from_start,
        };
        let Ok(input) = crate::stream_schedule_handlers::schedule_input(request) else {
            summary.skipped += 1;
            continue;
        };
        state.database.create_stream_schedule(input).await?;
        summary.created += 1;
    }
    Ok(Json(summary))
}

// -- automations ------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/automations/export", tag = "automations", responses((status = 200, body = AreaBundle)))]
pub async fn export_automations(
    State(state): State<AppState>,
) -> Result<Json<AreaBundle>, ApiError> {
    let categories = state.database.list_categories().await?;
    let targets = state.database.list_notification_targets().await?;
    let mut entries = Vec::new();
    for automation in state.database.list_automations().await? {
        let Some(definition) = state
            .database
            .automation_versions(automation.id)
            .await?
            .into_iter()
            .find(|version| version.version == automation.version)
        else {
            continue;
        };
        let actions = definition
            .actions
            .iter()
            .filter_map(|action| match action {
                rd_automation::Action::Webhook { target_id } => targets
                    .iter()
                    .find(|target| target.id == *target_id)
                    .map(|target| BundleAreaAction::Webhook {
                        target_name: target.name.clone(),
                    }),
                rd_automation::Action::Script { name } => {
                    Some(BundleAreaAction::Script { name: name.clone() })
                }
                rd_automation::Action::SetCategory { category_id } => categories
                    .iter()
                    .find(|category| category.id == *category_id)
                    .map(|category| BundleAreaAction::SetCategory {
                        category_name: category.name.clone(),
                    }),
                rd_automation::Action::PausePackage => Some(BundleAreaAction::PausePackage),
                rd_automation::Action::ResumePackage => Some(BundleAreaAction::ResumePackage),
            })
            .collect();
        entries.push(BundleAreaAutomation {
            name: automation.name,
            enabled: automation.enabled,
            trigger: definition.trigger,
            condition: definition.condition,
            actions,
        });
    }
    Ok(Json(AreaBundle {
        automations: Some(entries),
        ..AreaBundle::empty()
    }))
}

#[utoipa::path(post, path = "/api/v1/automations/import", tag = "automations", request_body = AreaBundle, responses((status = 200, body = ImportAreaSummary), (status = 400, body = crate::error::ErrorBody)))]
pub async fn import_automations(
    State(state): State<AppState>,
    Json(bundle): Json<AreaBundle>,
) -> Result<Json<ImportAreaSummary>, ApiError> {
    validate_header(&bundle)?;
    let entries = bundle
        .automations
        .ok_or_else(|| section_missing("automations"))?;
    check_section(entries.len())?;
    let categories = state.database.list_categories().await?;
    let targets = state.database.list_notification_targets().await?;
    // Same reason as the subscriptions import: one scan per entry against every stored name.
    let mut existing: HashSet<String> = state
        .database
        .list_automations()
        .await?
        .into_iter()
        .map(|automation| automation.name)
        .collect();
    let mut summary = ImportAreaSummary::default();
    for entry in entries {
        if existing.contains(&entry.name) {
            summary.skipped += 1;
            continue;
        }
        // An action pointing at something this instance does not have cannot be carried over,
        // and an automation missing one of its actions is not the automation somebody exported.
        // Counted as skipped rather than imported half-done.
        let mut actions = Vec::with_capacity(entry.actions.len());
        let mut unresolved = false;
        for action in &entry.actions {
            let resolved = match action {
                BundleAreaAction::Webhook { target_name } => targets
                    .iter()
                    .find(|target| &target.name == target_name)
                    .map(|target| rd_automation::Action::Webhook {
                        target_id: target.id,
                    }),
                BundleAreaAction::Script { name } => {
                    Some(rd_automation::Action::Script { name: name.clone() })
                }
                BundleAreaAction::SetCategory { category_name } => categories
                    .iter()
                    .find(|category| &category.name == category_name)
                    .map(|category| rd_automation::Action::SetCategory {
                        category_id: category.id,
                    }),
                BundleAreaAction::PausePackage => Some(rd_automation::Action::PausePackage),
                BundleAreaAction::ResumePackage => Some(rd_automation::Action::ResumePackage),
            };
            match resolved {
                Some(action) => actions.push(action),
                None => {
                    unresolved = true;
                    break;
                }
            }
        }
        if unresolved {
            summary.skipped += 1;
            continue;
        }
        let request = crate::automation_handlers::AutomationRequest {
            name: entry.name.clone(),
            enabled: entry.enabled,
            trigger: entry.trigger,
            condition: entry.condition,
            actions,
        };
        let Ok(input) = crate::automation_handlers::validated(request) else {
            summary.skipped += 1;
            continue;
        };
        state.database.upsert_automation(None, input).await?;
        existing.insert(entry.name);
        summary.created += 1;
    }
    Ok(Json(summary))
}
