//! Export and import of one configuration area at a time: subscriptions, streams, automations,
//! LinkFilter rules.
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
use rd_api_core::input_checks::BundleHeader;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

mod automations;
mod link_filters;
mod streams;
mod subscriptions;

pub use automations::*;
pub use link_filters::*;
pub use streams::*;
pub use subscriptions::*;

const BUNDLE_FORMAT: &str = "rdownloader-area-bundle";
const BUNDLE_VERSION: u32 = 1;

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
    /// The search an indexer subscription sends (RD-180-20); absent from an older bundle,
    /// which restores the empty search every subscription sent then.
    #[serde(default)]
    pub indexer_search: rd_core::IndexerSearch,
    /// Which release files a git-release subscription downloads (RD-190-13); absent from an
    /// older bundle, which has no such subscription.
    #[serde(default)]
    pub git_release: rd_core::GitReleaseOptions,
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
/// another instance. `Script` already carries a name, and the package and queue actions carry
/// nothing or a value that means the same everywhere (RD-1240-10).
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BundleAreaAction {
    Webhook {
        target_name: String,
    },
    Script {
        name: String,
    },
    SetCategory {
        category_name: String,
    },
    PausePackage,
    ResumePackage,
    SetPriority {
        priority: rd_core::DownloadPriority,
    },
    PauseQueue,
    StartQueue,
    ExtractPackage,
    Notify {
        target_name: String,
        message: String,
    },
    AddLinks {
        links: Vec<String>,
        #[serde(default)]
        destination: rd_automation::LinkDestination,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BundleAreaAutomation {
    pub name: String,
    pub enabled: bool,
    pub trigger: rd_automation::Trigger,
    /// A time trigger's schedule (RD-1240-10); absent for every other trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<rd_automation::Schedule>,
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
    /// The LinkFilter rules in their evaluation order (RD-1240-09).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_filters: Option<Vec<BundleAreaLinkFilter>>,
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
            link_filters: None,
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

/// What an area bundle says about itself; an older version is still read.
const BUNDLE_HEADER: BundleHeader = BundleHeader {
    format: BUNDLE_FORMAT,
    version: BUNDLE_VERSION,
    reads_older: true,
    format_code: "backup.format_unsupported",
    format_message: "This file is not an rDownloader area bundle",
    version_code: "backup.version_unsupported",
    version_message: "This bundle was written by a newer version",
};

fn validate_header(bundle: &AreaBundle) -> Result<(), ApiError> {
    BUNDLE_HEADER.check(&bundle.format, bundle.version)
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
