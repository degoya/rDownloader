//! The agent's own update (RD-1210-03): an agent installed without the service finds a newer
//! version itself, offers it in the tray and installs it on request — signature-checked, never
//! older than the running one, and taken back when the new version does not start in time.
//!
//! Beside the service nothing of this acts: the service's update replaces both programs, and the
//! agent restarts as the new one by the relaunch rule (`relaunch.rs`). Alone, the agent checks
//! [`FIRST_CHECK_DELAY`] after its start and then every [`CHECK_INTERVAL`] (`self-update.json`,
//! `rdownloader-capture update --auto-check on|off`), on the channel the service names on the
//! settings poll, stable without one. The format, the key and the install are `rd_update`'s; this
//! module is the agent's side of them:
//!
//! * [`watch`] — the check loop, the tray's "Install update to X" and the service's request;
//! * [`auto`] — "Install updates automatically" (RD-1240-27), off by default;
//! * [`update`] — `rdownloader-capture update`, the same for a terminal;
//! * [`apply_update`] — the updater, a copy of the agent outside its folder.
//!
//! What it says about itself goes to the service as a header on the settings poll ([`Shared`]),
//! so the update status lists it per connected agent. The agent's own text stays English
//! (RD-092-05).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_update::agent::AgentSetup;
use rd_update::agent::report::{AgentReport, CHANNEL_HEADER, REQUEST_HEADER, SelfUpdate};
use rd_update::{Channel, Floors, Offer, UpdateAction, is_newer};
use serde::{Deserialize, Serialize};

use crate::config;

mod auto;
mod check;
mod cli;
mod updater;
mod watch;

pub(crate) use auto::{Request, UpdateMenu};
pub(crate) use check::check_now;
pub(crate) use cli::{UpdateArgs, update};
pub(crate) use updater::{ApplyArgs, apply_update, recover_at_start, started};
pub(crate) use watch::spawn;

/// Beside `capture.json`: the agent's own switches.
const CONFIG_FILE: &str = "self-update.json";
/// Beside `capture.json`: what the last check found.
const STATE_FILE: &str = "self-update-state.json";
/// How long after its start the agent checks first: the service and the agent start together
/// at login, and the first answer of the settings poll names the channel.
pub(crate) const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// Between two checks: daily, as the service checks by default.
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The agent's own switches, in `self-update.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Config {
    /// Whether the agent checks after its start and then daily. On by default, as the service's
    /// check is: a check sends nothing but a request for a public file.
    pub(crate) check: bool,
    /// Whether the service may ask this agent to install an update. Off by default: no service,
    /// and no MCP tool behind one, installs software on this machine without its consent.
    pub(crate) allow_remote: bool,
    /// Whether a found update installs by itself (RD-1240-27). Off by default, for the same
    /// reason.
    pub(crate) auto_install: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check: true,
            allow_remote: false,
            auto_install: false,
        }
    }
}

/// What the last check found, kept for the next start and for `update`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct State {
    pub(crate) floors: Floors,
    pub(crate) last_checked: Option<DateTime<Utc>>,
    /// The stable code of what the last check refused or could not reach.
    pub(crate) last_error: Option<String>,
    pub(crate) offer: Option<Offer>,
    /// The channel the service last named, for an `update` run without it.
    pub(crate) service_channel: Option<Channel>,
}

impl Config {
    pub(crate) fn load(directory: &Path) -> Self {
        read_json(directory, CONFIG_FILE)
    }

    pub(crate) fn store(&self, directory: &Path) -> anyhow::Result<()> {
        write_json(directory, CONFIG_FILE, self)
    }
}

impl State {
    pub(crate) fn load(directory: &Path) -> Self {
        read_json(directory, STATE_FILE)
    }

    pub(crate) fn store(&self, directory: &Path) -> anyhow::Result<()> {
        write_json(directory, STATE_FILE, self)
    }

    /// The stored offer while it is newer than `running`: after an update, or one installed by
    /// hand, it is history.
    pub(crate) fn current_offer(&self, running: &str) -> Option<&Offer> {
        self.offer
            .as_ref()
            .filter(|offer| is_newer(&offer.version, running))
    }
}

/// A file of this module, or its defaults when there is none or it does not read.
fn read_json<T: Default + serde::de::DeserializeOwned>(directory: &Path, name: &str) -> T {
    match std::fs::read(directory.join(name)) {
        Ok(content) => serde_json::from_slice(&content).unwrap_or_else(|error| {
            tracing::warn!(%error, file = name, "the agent's update file does not read; starting from the defaults");
            T::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(error) => {
            tracing::warn!(%error, file = name, "the agent's update file cannot be read; starting from the defaults");
            T::default()
        }
    }
}

fn write_json<T: Serialize>(directory: &Path, name: &str, value: &T) -> anyhow::Result<()> {
    std::fs::create_dir_all(directory)?;
    config::write_atomically(directory, name, &serde_json::to_vec_pretty(value)?)
}

/// What the agent reports to the service.
pub(crate) fn report_of(
    setup: AgentSetup,
    settings: &Config,
    state: &State,
    running: &str,
    installing: Option<&str>,
) -> AgentReport {
    let offered = state
        .current_offer(running)
        .map(|offer| offer.version.clone());
    let (stands, offered) = match (setup, installing) {
        (AgentSetup::WithService, _) => (SelfUpdate::WithService, None),
        (_, Some(version)) => (SelfUpdate::Installing, Some(version.to_owned())),
        _ if !settings.check => (SelfUpdate::Disabled, None),
        _ if offered.is_some() => (SelfUpdate::Offered, offered),
        _ if state.last_error.is_some() => (SelfUpdate::Failed, None),
        _ if state.last_checked.is_some() => (SelfUpdate::Current, None),
        _ => (SelfUpdate::Unchecked, None),
    };
    AgentReport {
        state: stands,
        offered,
        remote_allowed: settings.allow_remote,
    }
}

/// The tray's entry for an offer: its text, and whether choosing it installs. Its shortcut reads
/// it too, on Linux as well (`install_refusal`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OfferEntry {
    pub(crate) label: String,
    pub(crate) enabled: bool,
}

/// Hands the tray its update entries. A callback, like `ActivitySink`.
pub(crate) type OfferSink = Arc<dyn Fn(UpdateMenu) + Send + Sync>;

/// The entry for `offer`: "Install update to X" where the agent installs it itself, the way to
/// get it otherwise; `None` beside the service, which offers it.
pub(crate) fn entry_for(setup: AgentSetup, offer: &Offer) -> Option<OfferEntry> {
    let version = &offer.version;
    let (label, enabled) = match setup.action(version)? {
        UpdateAction::Install if offer.artifact.is_some() => {
            (format!("Install update to {version}"), true)
        }
        UpdateAction::Command { command, .. } => (format!("Update to {version}: {command}"), false),
        UpdateAction::Install | UpdateAction::Download => (
            format!("Update to {version} is on the download page"),
            false,
        ),
    };
    Some(OfferEntry { label, enabled })
}

/// Why "Install update", clicked or pressed, installs nothing; `None` when it installs `offer`.
///
/// The shortcut obeys the entry (RD-1240-24): it is shown only with an offer and can be chosen only
/// where the agent installs the offer itself. A key press has no entry to grey out, so the text
/// is what the notification says instead.
pub(crate) fn install_refusal(setup: AgentSetup, offer: Option<&Offer>) -> Option<String> {
    let Some(offer) = offer else {
        return Some("No update of rDownloader Capture is offered".to_owned());
    };
    match entry_for(setup, offer) {
        Some(entry) if entry.enabled => None,
        Some(entry) => Some(entry.label),
        None => Some("The rDownloader service beside this agent updates it".to_owned()),
    }
}

/// Whether the service's request to install `requested` is carried out: only when this agent's
/// configuration allows it and `requested` is the version this agent offers itself.
///
/// # Errors
///
/// Why it is refused, for the log.
pub(crate) fn remote_decision(
    settings: &Config,
    state: &State,
    running: &str,
    requested: &str,
) -> Result<(), &'static str> {
    if !settings.allow_remote {
        return Err(
            "this agent does not let the service install updates (rdownloader-capture update --allow-remote on)",
        );
    }
    match state.current_offer(running) {
        Some(offer) if offer.version == requested => Ok(()),
        _ => Err("the version asked for is not the one this agent found newer"),
    }
}

/// What the agent's own update shares with the client: the report the settings poll carries,
/// and what the service's answer named.
#[derive(Clone, Default)]
pub(crate) struct Shared(Arc<Mutex<Exchange>>);

#[derive(Default)]
struct Exchange {
    report: Option<String>,
    channel: Option<Channel>,
    requested: Option<String>,
}

impl Shared {
    pub(crate) fn set_report(&self, report: &AgentReport) {
        if let Ok(mut exchange) = self.0.lock() {
            exchange.report = Some(report.to_header());
        }
    }

    /// The header value the next poll carries.
    pub(crate) fn report(&self) -> Option<String> {
        self.0.lock().ok()?.report.clone()
    }

    /// Reads the service's answer: its channel, and a request to install, if it made one.
    pub(crate) fn learn(&self, headers: &reqwest::header::HeaderMap) {
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
        };
        let channel = text(CHANNEL_HEADER).and_then(Channel::parse);
        let requested = text(REQUEST_HEADER)
            .filter(|version| rd_update::parse_version(version).is_some())
            .map(str::to_owned);
        if let Ok(mut exchange) = self.0.lock() {
            if channel.is_some() {
                exchange.channel = channel;
            }
            if requested.is_some() {
                exchange.requested = requested;
            }
        }
    }

    /// The channel the service named last.
    pub(crate) fn channel(&self) -> Option<Channel> {
        self.0.lock().ok()?.channel
    }

    /// The version the service asked this agent to install, once.
    pub(crate) fn take_request(&self) -> Option<String> {
        self.0.lock().ok()?.requested.take()
    }
}

#[cfg(test)]
#[path = "self_update_tests.rs"]
mod tests;
