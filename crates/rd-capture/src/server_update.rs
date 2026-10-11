//! The service's own update, offered in the tray (RD-1240-25).
//!
//! The agent reads `GET /api/v1/capture/server-update` -- the offered version, how it is
//! installed, whether this agent may install it, where an install stands -- and the tray shows
//! "Install server update X" while there is one. Choosing it, or pressing its shortcut
//! (`CaptureCommand::InstallServerUpdate`), installs it through the service's own install when
//! the agent was paired with "May install server updates" (`capture:server_update`), and opens
//! the update page in the browser when it was not. An installation the service does not replace
//! itself -- a package manager's, a container's -- shows its command, greyed out, and installs
//! nothing.
//!
//! An install the agent started is remembered beside `capture.json` ([`Pending`]), so its outcome
//! is announced even when the update replaced and restarted the agent too. While an install runs
//! the server line says so ([`View::updating`]). What is shown and said is decided here, on
//! every host; [`watch`] is the task around it. The agent's text stays English (RD-092-05).
//!
//! The same reading says whether a restart of the service is pending (RD-1240-32, [`restart`]):
//! the server line says so, and "Restart server" restarts it with the same right.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    client::{Detail, ServiceRefusal},
    config,
    self_update::OfferEntry,
};

mod restart;
mod watch;

pub(crate) use restart::RestartReading;
pub(crate) use watch::spawn;

/// Beside `capture.json`: the install this agent started and has not heard the end of.
const PENDING_FILE: &str = "server-update.json";
/// How long an install may stay silent before the agent stops waiting for its outcome.
const PENDING_LIMIT: chrono::Duration = chrono::Duration::minutes(30);
/// How long a second choice installs while downloads run, after the service said they do.
pub(crate) const CONFIRM_WINDOW: Duration = Duration::from_secs(120);
/// Between two readings while nothing is being installed: the service checks daily.
pub(crate) const READ_INTERVAL: Duration = Duration::from_secs(60);

/// What `GET /api/v1/capture/server-update` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct Reading {
    #[serde(default)]
    pub available: Option<Offered>,
    #[serde(default)]
    pub may_install: bool,
    #[serde(default)]
    pub install: Option<Installing>,
    /// Whether a restart is pending (RD-1240-32).
    #[serde(default)]
    pub restart: RestartReading,
}

/// What the tray's entries and shortcuts ask of the watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// "Install server update" (RD-1240-25).
    Install,
    /// "Restart server" (RD-1240-32).
    Restart,
}

/// The offered version and how it gets installed.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Offered {
    pub version: String,
    /// `install`, `download` or `command`.
    pub action: String,
    #[serde(default)]
    pub command: Option<String>,
}

/// Where an install stands.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Installing {
    pub state: String,
    pub target_version: String,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Installing {
    /// Whether it is over, one way or the other.
    pub(crate) fn ended(&self) -> bool {
        matches!(self.state.as_str(), "done" | "rolled_back" | "failed")
    }
}

/// The install this agent started, kept until its outcome is announced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub target: String,
    pub requested_at: DateTime<Utc>,
}

impl Pending {
    pub(crate) fn load(directory: &Path) -> Option<Self> {
        let content = std::fs::read(directory.join(PENDING_FILE)).ok()?;
        serde_json::from_slice(&content).ok()
    }

    pub(crate) fn store(&self, directory: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(directory)?;
        config::write_atomically(directory, PENDING_FILE, &serde_json::to_vec_pretty(self)?)
    }

    pub(crate) fn clear(directory: &Path) {
        match std::fs::remove_file(directory.join(PENDING_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "the server update's marker could not be removed"),
        }
    }
}

/// What the tray shows: the entry, and the version the server line names while it is installed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct View {
    pub entry: Option<OfferEntry>,
    pub updating: Option<String>,
    /// "Restart server", while a restart is pending and the agent may carry it out.
    pub restart: Option<OfferEntry>,
    /// A restart is pending: the server line says so.
    pub restart_pending: bool,
}

/// Hands the tray its view; a callback, like `ActivitySink`.
pub(crate) type ViewSink = Arc<dyn Fn(View) + Send + Sync>;

/// The version being installed: one the service reports running, or the one this agent started
/// and has not heard the end of -- the service is away while it restarts.
fn updating(reading: Option<&Reading>, pending: Option<&Pending>) -> Option<String> {
    reading
        .and_then(|reading| reading.install.as_ref())
        .filter(|install| !install.ended())
        .map(|install| install.target_version.clone())
        .or_else(|| pending.map(|pending| pending.target.clone()))
}

/// The tray's view of a reading.
pub(crate) fn view(reading: Option<&Reading>, pending: Option<&Pending>) -> View {
    let updating = updating(reading, pending);
    let entry = match &updating {
        Some(version) => Some(OfferEntry {
            label: format!("Installing server update {version}..."),
            enabled: false,
        }),
        None => reading
            .and_then(|reading| reading.available.as_ref())
            .map(offer_entry),
    };
    View {
        entry,
        updating,
        restart: restart::entry(reading),
        restart_pending: restart::pending(reading),
    }
}

fn offer_entry(offer: &Offered) -> OfferEntry {
    let version = &offer.version;
    let (label, enabled) = match offer.action.as_str() {
        "install" => (format!("Install server update {version}"), true),
        "command" => (command_hint(offer), false),
        _ => (
            format!("Server update {version} is on the update page"),
            true,
        ),
    };
    OfferEntry { label, enabled }
}

/// "Server update X: <command>" for an installation a package manager or container runtime
/// updates: shown, never installed from here.
fn command_hint(offer: &Offered) -> String {
    match &offer.command {
        Some(command) => format!("Server update {}: {command}", offer.version),
        None => format!(
            "Server update {}: update it with its package manager",
            offer.version
        ),
    }
}

/// What choosing the entry, or pressing its shortcut, does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Install through the service: the agent may.
    Install,
    /// Open the update page: the agent may not install, or the update is downloaded by hand.
    OpenPage,
    /// Nothing to do but say why.
    Tell(String),
}

pub(crate) fn choose(reading: Option<&Reading>, pending: Option<&Pending>) -> Choice {
    let Some(reading) = reading else {
        return Choice::Tell("rDownloader has not said yet whether it has an update".to_owned());
    };
    if let Some(version) = updating(Some(reading), pending) {
        return Choice::Tell(format!(
            "The server update to {version} is already being installed"
        ));
    }
    let Some(offer) = &reading.available else {
        return Choice::Tell("rDownloader is up to date".to_owned());
    };
    match offer.action.as_str() {
        "install" if reading.may_install => Choice::Install,
        "command" => Choice::Tell(command_hint(offer)),
        _ => Choice::OpenPage,
    }
}

/// The page the entry opens without the right: Settings > System > Updates.
pub(crate) fn update_page(service: &Url) -> Option<Url> {
    service.join("settings/system?tab=updates").ok()
}

/// The notification when the install has started.
pub(crate) fn started(version: &str) -> String {
    format!("Installing server update {version}; rDownloader restarts and is back in a moment")
}

/// What became of the install this agent started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    Waiting,
    /// Over; the notification says how.
    Ended(String),
}

pub(crate) fn resolve(
    pending: &Pending,
    reading: Option<&Reading>,
    now: DateTime<Utc>,
) -> Resolution {
    let outcome = reading
        .and_then(|reading| reading.install.as_ref())
        .filter(|install| install.ended() && install.target_version == pending.target);
    if let Some(install) = outcome {
        return Resolution::Ended(outcome_notice(install));
    }
    if now - pending.requested_at > PENDING_LIMIT {
        return Resolution::Ended(format!(
            "No word from the server update to {}; its outcome is on the update page",
            pending.target
        ));
    }
    Resolution::Waiting
}

fn outcome_notice(install: &Installing) -> String {
    let version = &install.target_version;
    let reason = install
        .reason
        .as_deref()
        .map(|code| format!(" ({code})"))
        .unwrap_or_default();
    match install.state.as_str() {
        "done" => format!("rDownloader was updated to {version}"),
        "rolled_back" => format!(
            "The server update to {version} did not start properly and was taken back{reason}"
        ),
        _ => format!(
            "The server update to {version} failed{reason}; the installed version keeps running"
        ),
    }
}

/// What a refused install says, and whether a second choice soon after installs anyway: the
/// service refused it only because downloads run.
pub(crate) fn refused(error: &anyhow::Error) -> (String, bool) {
    let Some(refusal) = error.downcast_ref::<ServiceRefusal>() else {
        return (
            "The server update was not started: rDownloader did not answer".to_owned(),
            false,
        );
    };
    match refusal.code() {
        Some("update.transfers_active") => {
            let running = match running_downloads(refusal) {
                Some(1) => "1 download is".to_owned(),
                Some(count) => format!("{count} downloads are"),
                None => "Downloads are".to_owned(),
            };
            (
                format!(
                    "{running} running. Choose \u{201c}Install server update\u{201d} again within \
                     two minutes to install anyway; they continue after the restart"
                ),
                true,
            )
        }
        Some("auth.scope_insufficient") => (
            "Pair the agent again with \u{201c}May install server updates\u{201d} to install from \
             the tray"
                .to_owned(),
            false,
        ),
        Some(code) => (format!("The server update was not started ({code})"), false),
        None => (
            format!(
                "The server update was not started (HTTP {})",
                refusal.status().as_u16()
            ),
            false,
        ),
    }
}

/// The `count` the service names with `update.transfers_active`.
fn running_downloads(refusal: &ServiceRefusal) -> Option<u64> {
    let Detail::Text(body) = refusal.detail() else {
        return None;
    };
    let count = serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("params")?
        .get("count")?
        .clone();
    count
        .as_u64()
        .or_else(|| count.as_str().and_then(|text| text.parse().ok()))
}

#[cfg(test)]
#[path = "server_update_tests.rs"]
mod tests;
