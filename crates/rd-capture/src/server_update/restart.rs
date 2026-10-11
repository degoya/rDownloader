//! The service's restart in the tray (RD-1240-32).
//!
//! `GET /api/v1/capture/server-update` says whether a restart is pending -- a plugin installed or
//! updated that runs only from the next start -- and the tray's second line says so. For an agent
//! paired with "May install server updates" (`capture:server_update`) the entry "Restart server"
//! appears beside the update entries; choosing it, or pressing its shortcut
//! (`CaptureCommand::RestartServer`), restarts the service through
//! `POST /api/v1/capture/server-update/restart`, the button of the web interface. Without the
//! right the entry stays hidden and the shortcut says why. What is shown and said is decided here;
//! the watch carries it out.

use serde::Deserialize;

use super::{Reading, running_downloads};
use crate::{client::ServiceRefusal, self_update::OfferEntry};

/// The restart part of the reading; the service says more (the reasons, how it restarts), which
/// the tray does not show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct RestartReading {
    /// Something waits for the next start.
    #[serde(default)]
    pub pending: bool,
    /// Whether a restart can begin now: no update installs, none runs already.
    #[serde(default)]
    pub can_restart: bool,
}

/// Whether the reading says a restart is pending.
pub(crate) fn pending(reading: Option<&Reading>) -> bool {
    reading.is_some_and(|reading| reading.restart.pending)
}

/// "Restart server", while a restart is pending and this agent may carry it out; greyed out while
/// it cannot begin.
pub(crate) fn entry(reading: Option<&Reading>) -> Option<OfferEntry> {
    let reading = reading?;
    (reading.restart.pending && reading.may_install).then(|| OfferEntry {
        label: "Restart server".to_owned(),
        enabled: reading.restart.can_restart,
    })
}

/// What choosing the entry, or pressing its shortcut, does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestartChoice {
    /// Restart through the service.
    Restart,
    /// Nothing to do but say why.
    Tell(String),
}

pub(crate) fn choose(reading: Option<&Reading>) -> RestartChoice {
    let Some(reading) = reading else {
        return RestartChoice::Tell(
            "rDownloader has not said yet whether a restart is pending".to_owned(),
        );
    };
    if !reading.may_install {
        return RestartChoice::Tell(
            "Pair the agent again with \u{201c}May install server updates\u{201d} to restart the \
             server from the tray"
                .to_owned(),
        );
    }
    if !reading.restart.pending {
        return RestartChoice::Tell("No restart of rDownloader is pending".to_owned());
    }
    if !reading.restart.can_restart {
        return RestartChoice::Tell(
            "rDownloader cannot restart right now: an update is being installed, or it is \
             restarting already"
                .to_owned(),
        );
    }
    RestartChoice::Restart
}

/// The notification once the restart began, by how the service comes back.
pub(crate) fn started(how: &str) -> String {
    match how {
        "self" => "rDownloader restarts and is back in a moment".to_owned(),
        "supervisor" => {
            "rDownloader restarts; its service manager starts it again in a moment".to_owned()
        }
        _ => {
            "rDownloader stops for its restart; start it again if it does not come back".to_owned()
        }
    }
}

/// What a refused restart says, and whether a second choice soon after restarts anyway: the
/// service refused it only because downloads run.
pub(crate) fn refused(error: &anyhow::Error) -> (String, bool) {
    let Some(refusal) = error.downcast_ref::<ServiceRefusal>() else {
        return (
            "rDownloader was not restarted: it did not answer".to_owned(),
            false,
        );
    };
    match refusal.code() {
        Some("restart.transfers_active") => {
            let running = match running_downloads(refusal) {
                Some(1) => "1 download is".to_owned(),
                Some(count) => format!("{count} downloads are"),
                None => "Downloads are".to_owned(),
            };
            (
                format!(
                    "{running} running. Choose \u{201c}Restart server\u{201d} again within two \
                     minutes to restart anyway; they continue after the restart"
                ),
                true,
            )
        }
        Some("auth.scope_insufficient") => (
            "Pair the agent again with \u{201c}May install server updates\u{201d} to restart the \
             server from the tray"
                .to_owned(),
            false,
        ),
        Some("restart.update_running") => (
            "An update of rDownloader is being installed; it restarts the server itself".to_owned(),
            false,
        ),
        Some("restart.already_restarting") => {
            ("rDownloader is restarting already".to_owned(), false)
        }
        Some(code) => (format!("rDownloader was not restarted ({code})"), false),
        None => (
            format!(
                "rDownloader was not restarted (HTTP {})",
                refusal.status().as_u16()
            ),
            false,
        ),
    }
}

#[cfg(test)]
#[path = "restart_tests.rs"]
mod tests;
