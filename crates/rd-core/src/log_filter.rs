//! The level the service logs at, and the area a person puts on debug for a while (RD-1260-12).
//!
//! Only the vocabulary lives here: `rd-api-core` carries the level in the settings document,
//! `rd-diagnostics` turns both into the `tracing` filter, and `rd-plugin-host` opens the span
//! a plugin's own log line is matched by. The filter itself is built in `rd-diagnostics`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The level the service records at: the console, the stored log and the trace export alike.
///
/// Four of the five `tracing` levels: `trace` is what the libraries underneath write and
/// nothing a person reads, so it is not offered.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ServiceLogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl ServiceLogLevel {
    /// The word `tracing`'s filter directives use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// An area of the service a person can put on debug for a while.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogDebugArea {
    /// The plugin host and its adapters, and every plugin's own lines; narrowed to one plugin
    /// with a plugin id.
    Plugin,
    /// Hosters and providers: the provider table and the resolver plugins' own lines.
    Provider,
    Usenet,
    Torrent,
    /// yt-dlp, ffmpeg, streamlink, gallery-dl and the managed tools.
    Media,
    Linkgrabber,
    Subscriptions,
    Http,
    Scheduler,
}

impl LogDebugArea {
    /// Every area, in the order the interface offers them.
    pub const ALL: [Self; 9] = [
        Self::Plugin,
        Self::Provider,
        Self::Usenet,
        Self::Torrent,
        Self::Media,
        Self::Linkgrabber,
        Self::Subscriptions,
        Self::Http,
        Self::Scheduler,
    ];

    /// The word the API and the settings use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Provider => "provider",
            Self::Usenet => "usenet",
            Self::Torrent => "torrent",
            Self::Media => "media",
            Self::Linkgrabber => "linkgrabber",
            Self::Subscriptions => "subscriptions",
            Self::Http => "http",
            Self::Scheduler => "scheduler",
        }
    }

    /// The `tracing` targets the area covers, as prefixes: a crate, or a module of one.
    #[must_use]
    pub const fn targets(self) -> &'static [&'static str] {
        match self {
            Self::Plugin => &[
                "rd_plugin_host",
                "rd_plugin_ext",
                "rd_plugin_transfer",
                "rd_plugin_api",
            ],
            Self::Provider => &["rd_provider_registry", "rd_api_core::hosters"],
            Self::Usenet => &["rd_usenet"],
            Self::Torrent => &["rd_torrent", "rd_api_core::torrent_intake"],
            Self::Media => &["rd_media", "rd_stream", "rd_gallery", "rd_tools"],
            Self::Linkgrabber => &["rd_collector", "rd_api_intake", "rd_siterules"],
            Self::Subscriptions => &["rd_subscription", "rd_api_core::subscription_service"],
            Self::Http => &["rd_http"],
            Self::Scheduler => &["rd_scheduler"],
        }
    }

    /// The plugin types whose own log lines belong to the area, matched through the span
    /// [`PLUGIN_LOG_SPAN`] the plugin host opens around each of them.
    #[must_use]
    pub const fn plugin_kinds(self) -> &'static [&'static str] {
        match self {
            Self::Provider => &["resolver"],
            Self::Linkgrabber => &["crawler", "intake"],
            Self::Media => &["stream-transform"],
            _ => &[],
        }
    }
}

/// The lengths a debug window may have, in minutes.
pub const LOG_DEBUG_MINUTES: [u32; 3] = [15, 30, 60];

/// One area on debug until a moment; after it the service falls back to its level by itself.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct LogDebugWindow {
    pub area: LogDebugArea,
    /// Only with [`LogDebugArea::Plugin`]: that one plugin's own lines instead of the host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    pub until: DateTime<Utc>,
}

impl LogDebugWindow {
    /// Whether the window is still open at `now`.
    #[must_use]
    pub fn is_open(&self, now: DateTime<Utc>) -> bool {
        now < self.until
    }
}

/// The level's slice of the `service.settings` blob (see
/// `rd_db::Database::service_settings_or_default`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogLevelSettings {
    pub log_level: ServiceLogLevel,
}

/// The settings key a running debug window is kept under, beside the settings document rather
/// than in it: it is a moment's state with an end, not configuration, so a saved settings page
/// or a restored settings backup neither brings one back nor ends one.
pub const LOG_DEBUG_SETTING_KEY: &str = "diagnostics.log_debug";

/// The span the plugin host opens around a plugin's own log line, with the fields `plugin_id`
/// and `kind` (the plugin type), so a filter directive can pick one plugin's lines.
pub const PLUGIN_LOG_SPAN: &str = "plugin_log";

/// Whether `id` may stand in a filter directive: what a plugin id is made of and nothing that
/// would end the directive (`,`, `]`, `}` or `=`).
#[must_use]
pub fn is_directive_safe_plugin_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::{LogDebugArea, ServiceLogLevel, is_directive_safe_plugin_id};

    #[test]
    fn the_level_reads_its_own_word_and_refuses_trace() {
        let level: ServiceLogLevel = serde_json::from_str("\"warn\"").expect("warn");
        assert_eq!(level, ServiceLogLevel::Warn);
        assert!(serde_json::from_str::<ServiceLogLevel>("\"trace\"").is_err());
        assert_eq!(ServiceLogLevel::default(), ServiceLogLevel::Info);
    }

    #[test]
    fn every_area_names_a_target() {
        for area in LogDebugArea::ALL {
            assert!(!area.targets().is_empty(), "{}", area.as_str());
            let word = serde_json::to_string(&area).expect("area");
            assert_eq!(word, format!("\"{}\"", area.as_str()));
        }
    }

    #[test]
    fn a_plugin_id_that_could_end_a_directive_is_refused() {
        assert!(is_directive_safe_plugin_id(
            "019d0000-0000-7000-8000-00000000abcd"
        ));
        for refused in ["", "a,b", "a]", "a}", "a=b", "a b"] {
            assert!(!is_directive_safe_plugin_id(refused), "{refused}");
        }
    }
}
