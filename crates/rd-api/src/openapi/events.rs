//! The two event streams in the document (RD-1480-09): what arrives under which name, for whom,
//! carrying what.
//!
//! The names come from `event_stream`, the areas from [`Scope::of_event`] -- the functions the
//! streams themselves use -- and the payloads from the `match` below, which a new kind does not
//! compile without.

use std::fmt::Write as _;

use axum::http::Method;
use rd_core::{EventKind, Scope};
use utoipa::openapi::OpenApi;

use super::operations;
use crate::event_stream::{EXPIRED_EVENT, LAGGED_EVENT, event_name};

/// Every kind, in the order `EventKind` declares them; a test holds the list complete.
pub(super) const KINDS: &[EventKind] = &[
    EventKind::DownloadProgress,
    EventKind::DownloadState,
    EventKind::PackageState,
    EventKind::CollectorChanged,
    EventKind::CollectorIntake,
    EventKind::CategoryChanged,
    EventKind::HotFolderChanged,
    EventKind::CaptureChanged,
    EventKind::AccountChanged,
    EventKind::AuthProfileChanged,
    EventKind::RemoteCredentialChanged,
    EventKind::ProxyChanged,
    EventKind::UsenetChanged,
    EventKind::PluginChanged,
    EventKind::PluginTrustChanged,
    EventKind::PluginCatalogChanged,
    EventKind::PostprocessCatalogChanged,
    EventKind::ManagedToolChanged,
    EventKind::StreamChanged,
    EventKind::SubscriptionChanged,
    EventKind::PostprocessProgress,
    EventKind::CaptchaChanged,
    EventKind::TorrentStats,
    EventKind::StorageCapacity,
    EventKind::BandwidthChanged,
    EventKind::PowerChanged,
    EventKind::NotificationChanged,
    EventKind::AutomationChanged,
    EventKind::ReconnectChanged,
    EventKind::RemoteJobChanged,
    EventKind::SiteRuleChanged,
    EventKind::SettingsChanged,
    EventKind::QueueStopMark,
    EventKind::QueueState,
    EventKind::System,
];

/// What `payload` carries. Every field may be missing and more may come: an event says what
/// changed, and the routes say how it stands.
fn payload(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::DownloadProgress => "`download_id`; `recording` about a stream recording",
        EventKind::DownloadState => {
            "`download_id`, `previous`, `state`, `failure`; about many files `count` and the \
             first 100 `download_ids`; `true` under the name of another change, `renamed` for one"
        }
        EventKind::PackageState => {
            "`package_id`, `previous`, `state`, `stage`, `percent`, `current`, `files`, \
             `failed_files`; `created`, `removed`, `reset` or `reordered_downloads`"
        }
        EventKind::CollectorChanged => {
            "`batch_id` and `candidate_count`, or `candidate_id`, or what else changed"
        }
        EventKind::CollectorIntake => {
            "`batch_id` or `nzb_import_id`, `candidate_count` or `file_count`, `package_count`, \
             `source`, `extension_notice`, `agent_toast`; `quiet_until` on the capture stream"
        }
        EventKind::CategoryChanged => {
            "`resource` (`category`, `category_rule`, `storage_root`) and `id`"
        }
        EventKind::HotFolderChanged => "`resource` and `id`",
        EventKind::CaptureChanged => {
            "`capture_token_id` with `issued`, `revoked`, `scopes_changed` or `limits_changed`, \
             or `resource`"
        }
        EventKind::AccountChanged => {
            "`resource` and `id`, or `entity` (`auth_flow`), `account_id` and `state`"
        }
        EventKind::AuthProfileChanged
        | EventKind::RemoteCredentialChanged
        | EventKind::StreamChanged => "`resource`",
        EventKind::ProxyChanged => "`resource` and `id`",
        EventKind::UsenetChanged => {
            "`nzb_import_id`, `nzb_segment_id` or `package_id` with `state` or `removed`, or \
             `resource`"
        }
        EventKind::PluginChanged => {
            "`resource` (`plugin`, `plugin_repository`, `plugin_updates`), `plugin_id` or \
             `repository_id`, `action`"
        }
        EventKind::PluginTrustChanged => "`resource` with `key_id` or `digest`",
        EventKind::PluginCatalogChanged | EventKind::PostprocessCatalogChanged => {
            "`resource`, `plugin_id`, `action`"
        }
        EventKind::ManagedToolChanged => {
            "`resource` (`tool`, `tool_manifest`) with `name` and `version`, or `sequence`"
        }
        EventKind::SubscriptionChanged => {
            "`resource`; after a poll `poll`, `subscription_id`, `found`, `accepted`, `skipped`"
        }
        EventKind::PostprocessProgress => {
            "`owner_id`, `kind`, `source_path`, with `stage`, `percent` and `current`, or `state`"
        }
        EventKind::CaptchaChanged => {
            "`pending` and `change`; on the capture stream only `widgets`, how many wait"
        }
        EventKind::TorrentStats => "`download_id`, `stats`",
        EventKind::StorageCapacity => "`blocked`, `change`, `target`, `name`, `path`",
        EventKind::BandwidthChanged => {
            "`entity` (`active`, `manual`, `profile`, `schedule`, `budget`) and what it names"
        }
        EventKind::PowerChanged => {
            "`state` (`pending`, `running`, `cancelled`) with `pending` and `runs_at`, or `action` \
             and `reason`"
        }
        EventKind::NotificationChanged => "`entity` and `id`",
        EventKind::AutomationChanged => "`automation_id`",
        EventKind::SettingsChanged => {
            "`fields`: the names of the changed settings, empty when unknown"
        }
        EventKind::ReconnectChanged => "`success`, `old_address`, `new_address`, `error`",
        EventKind::RemoteJobChanged => "`entity`, `id`, `account_id`, with `state` or `removed`",
        EventKind::SiteRuleChanged => "`resource` with `rule_id`, `removed`, `rules` or `key`",
        EventKind::QueueStopMark => "`action`, `download_id`, `package_id`",
        EventKind::QueueState => "`state` (`paused`, `resumed`, `finished`); `reason` when paused",
        EventKind::System => "not sent by this version",
    }
}

/// Writes the two streams' descriptions.
pub(super) fn describe(doc: &mut OpenApi) {
    let (web, capture) = (web_stream(), capture_stream());
    operations::each(doc, |path, method, operation| {
        if *method != Method::GET {
            return;
        }
        match path {
            "/api/v1/events" => operation.description = Some(web.clone()),
            "/api/v1/capture/events" => operation.description = Some(capture.clone()),
            _ => {}
        }
    });
}

fn web_stream() -> String {
    let mut text = format!(
        "Every event has an `id`, the name in the table as `event` and an `EventEnvelope` as \
         JSON in `data`. A subscriber is sent the events whose permission area its credential \
         holds; a session holds all of them.\n\n{SHARED}\n\n\
         | Event | Permission area | `payload` |\n| --- | --- | --- |\n"
    );
    for kind in KINDS {
        let _ = writeln!(
            text,
            "| `{}` | `{}` | {} |",
            event_name(kind),
            Scope::of_event(kind).as_str(),
            payload(kind)
        );
    }
    markers(&mut text);
    text
}

fn capture_stream() -> String {
    let mut text = format!(
        "The events a capture agent needs and no others: `{}` as the web stream sends it, and \
         `{}` reduced to how many widgets wait.\n\n{SHARED}\n",
        event_name(&EventKind::CollectorIntake),
        event_name(&EventKind::CaptchaChanged),
    );
    markers(&mut text);
    text
}

/// What holds for both streams.
const SHARED: &str = "`Last-Event-ID` resumes: the service first sends what it buffered after \
     that id. The buffer is in memory and ends with the service. The stream opens with the \
     `retry:` interval to reconnect at, and ends when the credential it was opened with stops \
     standing or the service stops.";

fn markers(text: &mut String) {
    let _ = write!(
        text,
        "\nTwo markers name nothing from the bus, carry no `id` and say that the client's view \
         is stale: `{LAGGED_EVENT}` (`dropped`, how many events this subscriber was too slow \
         for) and `{EXPIRED_EVENT}` (`last_event_id`, the id the buffer no longer reaches back \
         to). Read the lists again after either."
    );
}
