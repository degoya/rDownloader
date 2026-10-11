//! Following what the service has this agent set to: the clipboard pause (RD-1180-01), the
//! shortcuts (RD-1180-03) and the game mode and its switch (RD-1240-19, RD-1240-23).
//!
//! The service holds the settings, so the settings page, MCP, the tray and the `pause`/`resume`
//! commands all switch the same thing. The agent reads them on the five-second cadence of its
//! other polls and hands every change to whoever acts on it through a `watch` channel, which is
//! what applies a change made in the web interface without a restart.
//!
//! A copy is kept beside `capture.json`, so an agent that starts while the service is still
//! coming up -- both start at login -- starts paused when it was paused, instead of delivering
//! whatever is on the clipboard before the first answer arrives.
//!
//! A switch made here (the tray, a shortcut) is in force at once and sent until the service
//! takes it: a poll that answers in between must not switch it back, so while one is waiting the
//! poll is that request instead. Only a refusal ends the wait: the game mode switch needs queue
//! control, which an agent paired without it does not have.

use std::path::{Path, PathBuf};

use rd_core::{CaptureAgentSettings, CaptureShortcutReport};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    client::{CaptureClient, ServiceRefusal},
    config,
};

/// The cache's file name, beside `capture.json`.
const CACHE_FILE: &str = "agent-settings.json";

/// What the tray and the shortcuts ask of the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsRequest {
    /// "Pause clipboard watching": switch it to the other state.
    ToggleClipboard,
    /// "Pause while gaming": switch game mode on or off (RD-1240-23).
    ToggleGameMode,
    /// What registering the shortcuts came to, for the settings page.
    Report(CaptureShortcutReport),
}

/// The settings as last stored here, or the defaults when there is no readable copy.
pub(crate) fn load_cached(directory: Option<&Path>) -> CaptureAgentSettings {
    let Some(directory) = directory else {
        return CaptureAgentSettings::default();
    };
    match std::fs::read(directory.join(CACHE_FILE)) {
        Ok(content) => serde_json::from_slice(&content).unwrap_or_else(|error| {
            tracing::warn!(%error, "the stored agent settings do not read; starting from the defaults");
            CaptureAgentSettings::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            CaptureAgentSettings::default()
        }
        Err(error) => {
            tracing::warn!(%error, "the stored agent settings cannot be read; starting from the defaults");
            CaptureAgentSettings::default()
        }
    }
}

/// Stores the settings for the next start. A failure is a log line: the agent still follows the
/// service, it only starts from the defaults next time.
pub(crate) fn store_cached(directory: Option<&Path>, settings: &CaptureAgentSettings) {
    let Some(directory) = directory else {
        return;
    };
    let written = serde_json::to_vec_pretty(settings)
        .map_err(anyhow::Error::new)
        .and_then(|body| {
            std::fs::create_dir_all(directory)?;
            config::write_atomically(directory, CACHE_FILE, &body)
        });
    if let Err(error) = written {
        tracing::warn!(%error, "the agent settings could not be stored for the next start");
    }
}

/// The next call the loop makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    /// Read the settings.
    Read,
    /// Send the clipboard pause that is waiting; its answer is the settings.
    SendClipboard(bool),
    /// Send the game mode switch that is waiting; its answer is the settings.
    SendGameMode(bool),
}

/// The one rule of the loop: a switch made here waits to be sent, and until the service has it,
/// no reading switches it back.
#[derive(Debug, Default)]
pub(crate) struct Follower {
    clipboard: Option<bool>,
    game_mode: Option<bool>,
}

impl Follower {
    /// Switches the pause here, now, and queues it for the service. Returns the new settings.
    pub(crate) fn toggle_clipboard(
        &mut self,
        current: &CaptureAgentSettings,
    ) -> CaptureAgentSettings {
        let mut next = current.clone();
        next.clipboard_paused = !current.clipboard_paused;
        self.clipboard = Some(next.clipboard_paused);
        next
    }

    /// Switches game mode here, now, and queues it for the service; switched off, the game mode
    /// task lifts its hold at once. Returns the new settings.
    pub(crate) fn toggle_game_mode(
        &mut self,
        current: &CaptureAgentSettings,
    ) -> CaptureAgentSettings {
        let mut next = current.clone();
        next.game_mode.enabled = !current.game_mode.enabled;
        self.game_mode = Some(next.game_mode.enabled);
        next
    }

    pub(crate) fn next_call(&self) -> Call {
        match (self.clipboard, self.game_mode) {
            (Some(paused), _) => Call::SendClipboard(paused),
            (None, Some(enabled)) => Call::SendGameMode(enabled),
            (None, None) => Call::Read,
        }
    }

    /// The service answered `call` with `answer`; the settings to hold from now on.
    ///
    /// An answer to a send clears the switch only when it is still the one that went out: a
    /// second toggle while the first was on its way is sent next.
    pub(crate) fn answered(
        &mut self,
        call: Call,
        mut answer: CaptureAgentSettings,
    ) -> CaptureAgentSettings {
        self.sent(call);
        if let Some(paused) = self.clipboard {
            answer.clipboard_paused = paused;
        }
        if let Some(enabled) = self.game_mode {
            answer.game_mode.enabled = enabled;
        }
        answer
    }

    /// The service refused `call`: the switch it carried is not sent again, and the next reading
    /// shows what the service holds.
    pub(crate) fn refused(&mut self, call: Call) {
        self.sent(call);
    }

    /// Clears the switch `call` carried, while it is still the one waiting.
    fn sent(&mut self, call: Call) {
        match call {
            Call::SendClipboard(sent) if self.clipboard == Some(sent) => self.clipboard = None,
            Call::SendGameMode(sent) if self.game_mode == Some(sent) => self.game_mode = None,
            _ => {}
        }
    }
}

/// Whether the service turned a switch down for good, rather than not being reachable: an agent
/// paired without queue control may not switch game mode.
fn turned_down(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ServiceRefusal>()
        .is_some_and(|refusal| refusal.status() == reqwest::StatusCode::FORBIDDEN)
}

/// Keeps `settings` in step with the service until the agent stops.
pub(crate) async fn follow(
    client: CaptureClient,
    cancellation: CancellationToken,
    settings: watch::Sender<CaptureAgentSettings>,
    mut requests: mpsc::UnboundedReceiver<SettingsRequest>,
    cache: Option<PathBuf>,
) {
    let mut follower = Follower::default();
    let mut report: Option<CaptureShortcutReport> = None;
    let mut unreachable_logged = false;
    loop {
        let call = follower.next_call();
        let answer = match call {
            Call::Read => client.agent_settings().await,
            Call::SendClipboard(paused) => client.set_clipboard_paused(paused).await,
            Call::SendGameMode(enabled) => client.set_game_mode_enabled(enabled).await,
        };
        match answer {
            Ok(answer) => {
                unreachable_logged = false;
                let next = follower.answered(call, answer);
                adopt(&settings, next, cache.as_deref());
            }
            Err(error) if call != Call::Read && turned_down(&error) => {
                tracing::warn!(%error, ?call, "the service turned the switch down; following its settings again");
                follower.refused(call);
            }
            // Once per outage: the agent keeps what it holds and asks again on the next tick.
            Err(error) if !unreachable_logged => {
                tracing::warn!(%error, "could not read the agent settings; keeping the last ones");
                unreachable_logged = true;
            }
            Err(_) => {}
        }
        if let Some(waiting) = report.take()
            && let Err(error) = client.report_shortcuts(&waiting).await
        {
            tracing::debug!(%error, "the shortcut report did not arrive; sending it again");
            report = Some(waiting);
        }
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep(config::STATUS_POLL_INTERVAL) => {}
            Some(request) = requests.recv() => match request {
                SettingsRequest::ToggleClipboard => {
                    let next = follower.toggle_clipboard(&settings.borrow());
                    tracing::info!(paused = next.clipboard_paused, "clipboard watching switched here");
                    adopt(&settings, next, cache.as_deref());
                }
                SettingsRequest::ToggleGameMode => {
                    let next = follower.toggle_game_mode(&settings.borrow());
                    tracing::info!(enabled = next.game_mode.enabled, "game mode switched here");
                    adopt(&settings, next, cache.as_deref());
                }
                // Only the newest one is worth sending.
                SettingsRequest::Report(newest) => report = Some(newest),
            },
        }
    }
}

/// Holds `next`, telling the watchers and storing it only when something changed.
fn adopt(
    settings: &watch::Sender<CaptureAgentSettings>,
    next: CaptureAgentSettings,
    cache: Option<&Path>,
) {
    let changed = settings.send_if_modified(|held| {
        if *held == next {
            return false;
        }
        *held = next.clone();
        true
    });
    if changed {
        store_cached(cache, &next);
    }
}

#[cfg(test)]
mod tests {
    use rd_core::CaptureAgentSettings;

    use super::{Call, Follower, load_cached, store_cached};

    fn paused(value: bool) -> CaptureAgentSettings {
        CaptureAgentSettings {
            clipboard_paused: value,
            ..CaptureAgentSettings::default()
        }
    }

    /// The pause holds over a restart of the agent, also when the service is not answering yet
    /// at the moment it starts (RD-1180-01).
    #[test]
    fn the_pause_survives_a_restart_of_the_agent() {
        let directory =
            std::env::temp_dir().join(format!("rd-capture-agent-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            load_cached(Some(&directory)),
            CaptureAgentSettings::default(),
            "nothing stored yet: watching, the default shortcuts"
        );
        // The directory does not exist before the first pairing wrote it; storing creates it.
        store_cached(Some(&directory), &paused(true));
        assert!(load_cached(Some(&directory)).clipboard_paused);
        assert!(
            !directory.join("agent-settings.json.new").exists(),
            "the copy is replaced as a whole"
        );

        std::fs::write(directory.join("agent-settings.json"), b"{ truncated").expect("write");
        assert_eq!(
            load_cached(Some(&directory)),
            CaptureAgentSettings::default(),
            "a copy that does not read is the defaults, not a failed start"
        );
        assert_eq!(load_cached(None), CaptureAgentSettings::default());
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    /// The tray's switch is in force at once and is not switched back by a poll that the service
    /// answered before it had the switch.
    #[test]
    fn a_switch_made_here_is_not_undone_by_the_next_reading() {
        let mut follower = Follower::default();
        assert_eq!(follower.next_call(), Call::Read);
        let held = follower.toggle_clipboard(&paused(false));
        assert!(held.clipboard_paused, "in force at once");
        assert_eq!(
            follower.next_call(),
            Call::SendClipboard(true),
            "sent before anything is read"
        );

        // A reading that crossed the switch on its way still says "watching".
        let crossed = follower.answered(Call::Read, paused(false));
        assert!(
            crossed.clipboard_paused,
            "a reading does not switch it back"
        );
        assert_eq!(follower.next_call(), Call::SendClipboard(true));

        // The service took it.
        let taken = follower.answered(Call::SendClipboard(true), paused(true));
        assert!(taken.clipboard_paused);
        assert_eq!(follower.next_call(), Call::Read);

        // From now on the service decides again: the settings page resumed it.
        let resumed = follower.answered(Call::Read, paused(false));
        assert!(!resumed.clipboard_paused);
    }

    #[test]
    fn a_second_switch_while_the_first_is_on_its_way_is_sent_next() {
        let mut follower = Follower::default();
        let first = follower.toggle_clipboard(&paused(false));
        let second = follower.toggle_clipboard(&first);
        assert!(!second.clipboard_paused);
        // The answer to the first send arrives after the second switch.
        let answer = follower.answered(Call::SendClipboard(true), paused(true));
        assert!(!answer.clipboard_paused, "the newer switch holds");
        assert_eq!(follower.next_call(), Call::SendClipboard(false));
    }

    fn game_mode(enabled: bool) -> CaptureAgentSettings {
        let mut settings = CaptureAgentSettings::default();
        settings.game_mode.processes = vec!["game.exe".to_owned()];
        settings.game_mode.enabled = enabled;
        settings
    }

    /// The tray's "Pause while gaming" (RD-1240-23) follows the same rule as the clipboard pause,
    /// and the clipboard switch waiting beside it goes out first without losing it.
    #[test]
    fn the_game_mode_switch_holds_until_the_service_has_it() {
        let mut follower = Follower::default();
        let off = follower.toggle_game_mode(&game_mode(true));
        assert!(!off.game_mode.enabled, "in force at once");
        assert_eq!(off.game_mode.processes, ["game.exe"], "the programs stay");
        let both = follower.toggle_clipboard(&off);
        assert_eq!(follower.next_call(), Call::SendClipboard(true));

        let mut answer = game_mode(true);
        answer.clipboard_paused = true;
        let held = follower.answered(Call::SendClipboard(true), answer);
        assert_eq!(
            held, both,
            "the clipboard answer does not switch game mode back on"
        );
        assert_eq!(follower.next_call(), Call::SendGameMode(false));

        let crossed = follower.answered(Call::Read, game_mode(true));
        assert!(
            !crossed.game_mode.enabled,
            "a reading does not switch it back"
        );
        let taken = follower.answered(Call::SendGameMode(false), game_mode(false));
        assert!(!taken.game_mode.enabled);
        assert_eq!(follower.next_call(), Call::Read);
    }

    /// An agent paired without queue control may not switch game mode: the refusal ends the
    /// wait, and the next reading shows the service's state again.
    #[test]
    fn a_refused_switch_is_not_sent_again() {
        let mut follower = Follower::default();
        follower.toggle_game_mode(&game_mode(true));
        follower.refused(Call::SendGameMode(false));
        assert_eq!(follower.next_call(), Call::Read);
        let read = follower.answered(Call::Read, game_mode(true));
        assert!(read.game_mode.enabled, "the service's state again");
    }

    #[test]
    fn only_a_forbidden_answer_turns_a_switch_down() {
        let refusal = |status| {
            anyhow::Error::from(crate::client::ServiceRefusal::new(
                "game mode switch",
                status,
                r#"{"code":"auth.scope_insufficient"}"#,
            ))
        };
        assert!(super::turned_down(&refusal(reqwest::StatusCode::FORBIDDEN)));
        assert!(!super::turned_down(&refusal(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        )));
        assert!(!super::turned_down(&anyhow::anyhow!("connection refused")));
    }
}
