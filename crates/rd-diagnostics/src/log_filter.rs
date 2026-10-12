//! The one filter in front of the console, the stored log and the trace export (RD-1260-12).
//!
//! The binary installs [`layer`] as the subscriber's first layer, so every layer after it sees
//! only what the filter lets through: what the console shows, the store keeps and the export
//! sends stay the same set. The filter sits in a [`reload::Layer`], and [`Controller::apply`]
//! swaps it while the service runs — a level chosen in the interface holds from the next line,
//! without a restart.
//!
//! Three inputs decide it:
//!
//! * the **level** from the settings document, `info` on a fresh installation;
//! * a **debug window**: one area on `debug` until a moment, kept apart from the settings
//!   (`rd_core::LOG_DEBUG_SETTING_KEY`) so a start inside the window reopens it and a start
//!   after it does not ([`reopened`]). A timer closes it at its end;
//! * **`RUST_LOG`**, which wins: set when the service started, it is the filter for good, and
//!   [`Controller::apply`] records what was asked without touching the filter.
//!
//! The directives cover this application's own targets only (`rdownloader`, `rd_*`): what the
//! libraries underneath write was never shown and is not now.

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use rd_core::{LogDebugWindow, PLUGIN_LOG_SPAN, ServiceLogLevel};
use serde::Serialize;
use tracing_subscriber::{EnvFilter, Registry, reload};

/// The filter layer the binary installs first.
pub type FilterLayer = reload::Layer<EnvFilter, Registry>;

/// The filter directives for a level and an optional debug window, as of `now`.
///
/// A window that is closed at `now` adds nothing. A plugin window with a plugin id picks that
/// plugin's own lines through the span the plugin host opens around each of them; any other
/// window puts its targets and its plugin types on `debug`.
#[must_use]
pub fn directives(
    level: ServiceLogLevel,
    window: Option<&LogDebugWindow>,
    now: DateTime<Utc>,
) -> String {
    let level = level.as_str();
    let mut parts = vec![format!("rdownloader={level}"), format!("rd_={level}")];
    if let Some(window) = window.filter(|window| window.is_open(now)) {
        let plugin = window
            .plugin_id
            .as_deref()
            .filter(|id| rd_core::is_directive_safe_plugin_id(id));
        match plugin {
            Some(id) => parts.push(format!("[{PLUGIN_LOG_SPAN}{{plugin_id={id}}}]=debug")),
            None => {
                parts.extend(
                    window
                        .area
                        .targets()
                        .iter()
                        .map(|target| format!("{target}=debug")),
                );
                parts.extend(
                    window
                        .area
                        .plugin_kinds()
                        .iter()
                        .map(|kind| format!("[{PLUGIN_LOG_SPAN}{{kind={kind}}}]=debug")),
                );
            }
        }
    }
    parts.join(",")
}

/// The window a start reopens: the stored one while it is still open at `now`, else none.
#[must_use]
pub fn reopened(stored: Option<LogDebugWindow>, now: DateTime<Utc>) -> Option<LogDebugWindow> {
    stored.filter(|window| window.is_open(now))
}

/// What the filter is right now, for the interface, the API and the diagnostic bundle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FilterState {
    /// The level the settings ask for.
    pub level: ServiceLogLevel,
    /// The open debug window, if any.
    pub debug: Option<LogDebugWindow>,
    /// `RUST_LOG` as the service found it at start; while set it is the filter, and the level
    /// and the window above are recorded but not in force.
    pub rust_log: Option<String>,
}

/// Why [`Controller::apply`] did not change the filter.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// `RUST_LOG` was set at start and stays the filter.
    LockedByEnvironment,
    /// The directives did not parse or the filter could not be swapped.
    Filter(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockedByEnvironment => formatter.write_str("RUST_LOG decides the log filter"),
            Self::Filter(reason) => write!(formatter, "the log filter was not changed: {reason}"),
        }
    }
}

impl std::error::Error for ApplyError {}

#[derive(Default)]
struct Inner {
    state: FilterState,
    handle: Option<reload::Handle<EnvFilter, Registry>>,
    /// Bumped by every apply, so the timer of a window that was replaced or ended does nothing
    /// when it fires.
    generation: u64,
}

/// The level, the debug window and the handle that swaps the filter.
#[derive(Clone, Default)]
pub struct Controller {
    inner: Arc<Mutex<Inner>>,
}

impl Controller {
    /// A controller of its own and the filter layer it swaps, for a test that installs its own
    /// subscriber. The service uses [`layer`] and [`controller`].
    #[must_use]
    pub fn with_layer(rust_log: Option<String>) -> (Self, FilterLayer) {
        let controller = Self::default();
        let layer = controller.attach(rust_log);
        (controller, layer)
    }

    /// A new filter layer this controller swaps from now on. `rust_log` is the value of
    /// `RUST_LOG` at start; one that parses is the filter for good, one that does not is
    /// ignored, as it always was.
    fn attach(&self, rust_log: Option<String>) -> FilterLayer {
        let rust_log = rust_log.filter(|value| !value.trim().is_empty());
        let environment = rust_log
            .as_deref()
            .and_then(|value| EnvFilter::try_new(value).ok());
        let locked = environment.is_some();
        let filter = environment.unwrap_or_else(|| {
            EnvFilter::new(directives(ServiceLogLevel::default(), None, Utc::now()))
        });
        let (layer, handle) = reload::Layer::new(filter);
        if let Ok(mut inner) = self.inner.lock() {
            inner.generation = inner.generation.wrapping_add(1);
            inner.handle = Some(handle);
            inner.state = FilterState {
                rust_log: if locked { rust_log } else { None },
                ..FilterState::default()
            };
        }
        layer
    }

    /// The filter as it is now; a window whose end passed reads as none.
    #[must_use]
    pub fn state(&self) -> FilterState {
        let Ok(inner) = self.inner.lock() else {
            return FilterState::default();
        };
        let mut state = inner.state.clone();
        state.debug = reopened(state.debug, Utc::now());
        state
    }

    /// Sets the level and the debug window and swaps the filter, from the next line on.
    ///
    /// A window is closed by a timer at its end; inside a Tokio runtime only, which is where
    /// the service and every caller of this run. With `RUST_LOG` set the values are recorded,
    /// so the interface can say what would apply, and [`ApplyError::LockedByEnvironment`] is
    /// returned.
    pub fn apply(
        &self,
        level: ServiceLogLevel,
        window: Option<LogDebugWindow>,
    ) -> Result<FilterState, ApplyError> {
        let now = Utc::now();
        let window = reopened(window, now);
        let (generation, locked) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| ApplyError::Filter("the filter state is poisoned".to_owned()))?;
            inner.generation = inner.generation.wrapping_add(1);
            inner.state.level = level;
            inner.state.debug = window.clone();
            let locked = inner.state.rust_log.is_some();
            if !locked && let Some(handle) = &inner.handle {
                let filter = EnvFilter::try_new(directives(level, window.as_ref(), now))
                    .map_err(|error| ApplyError::Filter(error.to_string()))?;
                handle
                    .reload(filter)
                    .map_err(|error| ApplyError::Filter(error.to_string()))?;
            }
            (inner.generation, locked)
        };
        if let Some(window) = &window {
            self.close_at_end(generation, window.until);
        }
        if locked {
            return Err(ApplyError::LockedByEnvironment);
        }
        Ok(self.state())
    }

    /// Falls back to the level at `until`, unless something applied in between.
    fn close_at_end(&self, generation: u64, until: DateTime<Utc>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let controller = self.clone();
        runtime.spawn(async move {
            let wait = (until - Utc::now()).to_std().unwrap_or_default();
            tokio::time::sleep(wait).await;
            let level = {
                let Ok(inner) = controller.inner.lock() else {
                    return;
                };
                if inner.generation != generation {
                    return;
                }
                inner.state.level
            };
            // Locked or not, the window is over; the error only says the filter is RUST_LOG's.
            let _ = controller.apply(level, None);
        });
    }
}

/// The service's one controller.
#[must_use]
pub fn controller() -> &'static Controller {
    static CONTROLLER: OnceLock<Controller> = OnceLock::new();
    CONTROLLER.get_or_init(Controller::default)
}

/// The filter layer for the service's subscriber, with `RUST_LOG` read from the environment;
/// [`controller`] swaps it from then on.
///
/// Called once, by the binary, before anything logs. A second call replaces the handle the
/// controller holds, which is what a test installing its own subscriber wants.
#[must_use]
pub fn layer() -> FilterLayer {
    install(std::env::var(EnvFilter::DEFAULT_ENV).ok())
}

/// [`layer`] with `RUST_LOG` given rather than read, so a test can stand for an environment
/// without changing the process's own.
#[must_use]
pub fn install(rust_log: Option<String>) -> FilterLayer {
    controller().attach(rust_log)
}

/// The debug window the database keeps, or none: a missing, emptied or unreadable entry is no
/// window, said in the log when it is unreadable.
pub async fn stored_window(database: &rd_db::Database) -> Option<LogDebugWindow> {
    let value = match database.get_setting(rd_core::LOG_DEBUG_SETTING_KEY).await {
        Ok(value) => value?,
        Err(error) => {
            tracing::warn!(%error, "the debug window could not be read");
            return None;
        }
    };
    match serde_json::from_value::<Option<LogDebugWindow>>(value) {
        Ok(window) => window,
        Err(error) => {
            tracing::warn!(%error, "the stored debug window is unreadable and was ignored");
            None
        }
    }
}

/// Applies the level the settings hold and a debug window still open, at start: called as soon
/// as the database is open, so only the lines before it are written at `info`.
pub async fn apply_stored(database: &rd_db::Database) {
    let settings: rd_core::LogLevelSettings = database
        .service_settings_or_default()
        .await
        .unwrap_or_default();
    let window = reopened(stored_window(database).await, Utc::now());
    match controller().apply(settings.log_level, window) {
        Ok(_) | Err(ApplyError::LockedByEnvironment) => {}
        Err(error) => tracing::warn!(%error, "the stored log level was not applied"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rd_core::{LogDebugArea, LogDebugWindow, ServiceLogLevel};

    use super::{directives, reopened};

    fn window(area: LogDebugArea, minutes: i64) -> LogDebugWindow {
        LogDebugWindow {
            area,
            plugin_id: None,
            until: Utc::now() + Duration::minutes(minutes),
        }
    }

    #[test]
    fn the_level_covers_the_applications_own_targets() {
        assert_eq!(
            directives(ServiceLogLevel::Warn, None, Utc::now()),
            "rdownloader=warn,rd_=warn"
        );
    }

    #[test]
    fn an_open_window_adds_its_area_and_a_closed_one_nothing() {
        let usenet = window(LogDebugArea::Usenet, 15);
        let now = Utc::now();
        assert_eq!(
            directives(ServiceLogLevel::Info, Some(&usenet), now),
            "rdownloader=info,rd_=info,rd_usenet=debug"
        );
        let later = now + Duration::minutes(16);
        assert_eq!(
            directives(ServiceLogLevel::Info, Some(&usenet), later),
            "rdownloader=info,rd_=info"
        );
    }

    #[test]
    fn a_window_on_one_plugin_picks_its_span_and_an_unsafe_id_falls_back_to_the_area() {
        let mut plugin = window(LogDebugArea::Plugin, 30);
        plugin.plugin_id = Some("019d0000-0000-7000-8000-00000000abcd".to_owned());
        assert!(
            directives(ServiceLogLevel::Info, Some(&plugin), Utc::now())
                .ends_with(",[plugin_log{plugin_id=019d0000-0000-7000-8000-00000000abcd}]=debug")
        );
        plugin.plugin_id = Some("x],rd_=trace".to_owned());
        let fallback = directives(ServiceLogLevel::Info, Some(&plugin), Utc::now());
        assert!(!fallback.contains("trace"), "{fallback}");
        assert!(fallback.contains("rd_plugin_host=debug"), "{fallback}");
    }

    #[test]
    fn a_resolver_window_reaches_the_resolver_plugins_lines() {
        let provider = window(LogDebugArea::Provider, 15);
        assert!(
            directives(ServiceLogLevel::Info, Some(&provider), Utc::now())
                .contains("[plugin_log{kind=resolver}]=debug")
        );
    }

    #[test]
    fn a_start_inside_the_window_reopens_it_and_one_after_does_not() {
        let stored = window(LogDebugArea::Torrent, 60);
        let now = Utc::now();
        assert_eq!(
            reopened(Some(stored.clone()), now + Duration::minutes(59)),
            Some(stored.clone())
        );
        assert_eq!(reopened(Some(stored), now + Duration::minutes(61)), None);
        assert_eq!(reopened(None, now), None);
    }
}
