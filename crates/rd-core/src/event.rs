use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::EventId;

/// Event channel families exposed through SSE.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    DownloadProgress,
    DownloadState,
    PackageState,
    CollectorChanged,
    /// Links were taken into the LinkGrabber (`collector.intake`). Distinct from
    /// `CollectorChanged`, which also fires for every later edit of a candidate; this one marks
    /// one import arriving and is what a desktop notification hangs off.
    CollectorIntake,
    CategoryChanged,
    HotFolderChanged,
    CaptureChanged,
    AccountChanged,
    /// An auth profile was created, changed, enabled/disabled or removed.
    AuthProfileChanged,
    /// A stored FTP/SFTP login or a trusted SSH host key was added, changed or removed.
    RemoteCredentialChanged,
    ProxyChanged,
    UsenetChanged,
    /// A plugin was installed, enabled, disabled or removed (`plugin.changed`) -- the
    /// administration axis, matching the `Admin`-scoped `/api/v1/plugins*` routes.
    ///
    /// Trust decisions are deliberately not this kind; they announce themselves as
    /// [`PluginTrustChanged`](Self::PluginTrustChanged).
    PluginChanged,
    /// A plugin signing key was trusted or revoked, or a package digest was withdrawn or
    /// reinstated (`plugin_trust.changed`).
    ///
    /// Separate from [`PluginChanged`](Self::PluginChanged) because an event's scope follows
    /// the scope of the write that produces it. These four writes sit behind the
    /// `Secrets`-scoped `/api/v1/plugins/keys*` and `/api/v1/plugins/revocations*` routes, and
    /// their payloads carry a `key_id` or a `digest` -- identifiers out of `Secrets`-scoped
    /// tables. Announcing them as `Admin` was wrong in both directions: the token that made
    /// the write never saw the event it caused, and the identifiers were delivered to
    /// subscribers who may not read those tables.
    PluginTrustChanged,
    /// What the installed plugins offer to *configuration*: the provider registry and the
    /// notification destinations (`plugin_catalog.changed`).
    ///
    /// The same write produces this, [`PluginChanged`](Self::PluginChanged) and
    /// [`PostprocessCatalogChanged`](Self::PostprocessCatalogChanged), and that is not
    /// redundancy. A subscriber is handed an event only when it holds the event's exact scope,
    /// and the lists this invalidates are read at three different ones — so a single kind
    /// would reach a third of the screens that go stale and leave the rest silently wrong. An
    /// event's scope follows the scope of the read it invalidates.
    PluginCatalogChanged,
    /// What the installed plugins offer to the *queue*: post-processing steps and upload
    /// destinations, both read under `/api/v1/postprocess/` (`postprocess_catalog.changed`).
    /// See [`PluginCatalogChanged`](Self::PluginCatalogChanged) for why this is its own kind.
    PostprocessCatalogChanged,
    /// An installed external tool version, or the accepted tool manifest, changed
    /// (`managed_tool.changed`). The payload names the tool and its version, or the manifest
    /// sequence, and never the download URL or the digest behind it.
    ManagedToolChanged,
    /// Stream channel added/updated/removed or its live state changed.
    StreamChanged,
    /// A subscription, its items or its poll history changed (RD-080-07).
    SubscriptionChanged,
    /// Stage/percent of a running post-processing step (`postprocess.progress`).
    PostprocessProgress,
    /// A captcha is waiting to be solved, or stopped waiting. Transient by nature, so these
    /// events are broadcast to live clients only and never persisted.
    CaptchaChanged,
    /// Aggregate torrent counters; broadcast only, never persisted.
    TorrentStats,
    /// A storage root started or stopped blocking work because of its free-space
    /// threshold (`storage.capacity`).
    StorageCapacity,
    /// A bandwidth profile, its schedule or the active profile changed
    /// (`bandwidth.changed`).
    BandwidthChanged,
    /// A queue completion action is counting down, or the power/network context changed
    /// (`power.changed`). Broadcast only, never persisted.
    PowerChanged,
    /// A notification target, rule or delivery changed (`notification.changed`).
    NotificationChanged,
    /// An automation was created, edited, enabled, disabled or removed
    /// (`automation.changed`).
    AutomationChanged,
    /// A reconnect attempt finished (`reconnect.changed`). Broadcast only, never persisted.
    ReconnectChanged,
    /// A job running at a provider was created, advanced, answered or removed
    /// (`remote_job.changed`, RD-107-06).
    RemoteJobChanged,
    /// A user-written site rule was written or removed (`site_rule.changed`, RD-110-04).
    /// The payload names the rule id and never carries the rule body.
    SiteRuleChanged,
    System,
}

/// Persistable event envelope.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct EventEnvelope {
    pub id: EventId,
    pub kind: EventKind,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

impl EventEnvelope {
    /// Creates an event stamped with the current time.
    #[must_use]
    pub fn new(kind: EventKind, payload: Value) -> Self {
        Self {
            id: EventId::new(),
            kind,
            occurred_at: Utc::now(),
            payload,
        }
    }
}
