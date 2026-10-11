//! Game mode (RD-1240-19): while a full-screen program is in front or a named process runs, the
//! agent pauses the queue or switches on a bandwidth profile, and lifts it once that is over.
//!
//! The settings come with the agent's others (`agent_settings`, `rd_core::CaptureGameMode`); what
//! a hold does the service reads from them too, so the agent only says when. Each hold is timed
//! and renewed while the trigger lasts (`rd_core::GAME_MODE_HOLD_MINUTES`): an agent that
//! crashes or sleeps leaves nothing behind for longer than that.
//!
//! The agent lifts only what it set itself. The service holds over nothing or over the agent's
//! own hold only, and lifts only the hold ending when the agent's last answer said; a pause
//! somebody resumed while the game runs stays resumed until the game is over ([`Guard`]).
//!
//! Switched off (the tray's "Pause while gaming", RD-1240-23), the agent looks no more and lifts
//! its hold at once: a change of the settings wakes the loop instead of waiting for the next look.
//!
//! [`Guard`] decides and `detect` looks, both ungated so they are tested on Linux; the
//! system calls behind `detect` are the only platform parts.

use chrono::{DateTime, Utc};
use rd_core::{CaptureAgentSettings, CaptureGameMode, GameModeAction};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    client::CaptureClient,
    config,
    supervision::{NoticeSink, supervised},
};

mod detect;

pub(crate) use detect::Seen;

/// Why game mode cannot be switched while it watches for nothing: the tray's greyed entry and a
/// pressed shortcut say this (RD-1240-23).
pub(crate) const NOTHING_SET_UP: &str =
    "game mode watches for no program and not for full screen; set it up in Settings first";
/// Why an agent paired without queue control cannot switch it.
#[cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]
pub(crate) const NO_QUEUE_CONTROL: &str =
    "this agent was paired without queue control; pair it again to switch game mode";
/// Why it cannot be switched before the service said whether the agent may control the queue.
#[cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]
pub(crate) const QUEUE_CONTROL_UNKNOWN: &str =
    "the service has not said yet whether this agent may control the queue";

/// What the service answered to a hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// Set or renewed until then.
    Until(DateTime<Utc>),
    /// Not set: somebody else's pause or switch holds, the agent's own was ended or changed, or
    /// game mode is off on the service.
    Refused,
}

/// Why the agent steps aside, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    FullScreen,
    Process(String),
}

/// What the desktop shows against what the settings watch for; nothing while game mode is
/// switched off.
pub(crate) fn trigger(mode: &CaptureGameMode, seen: &Seen) -> Option<Trigger> {
    if !mode.enabled {
        return None;
    }
    if mode.full_screen && seen.full_screen {
        return Some(Trigger::FullScreen);
    }
    mode.running_process(&seen.processes)
        .map(|name| Trigger::Process(name.to_owned()))
}

/// The agent's side of a hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Guard {
    /// Nothing held.
    #[default]
    Idle,
    /// The agent's own hold, ending at `until`, of the kind `action`.
    Holding {
        until: DateTime<Utc>,
        action: GameModeAction,
    },
    /// Somebody ended or changed the agent's hold while the trigger lasted: theirs from now on,
    /// until the trigger is over.
    Yielded,
}

/// The call the loop makes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Wait,
    /// A new hold, or the renewal of the agent's own ending at `renews`.
    Hold {
        renews: Option<DateTime<Utc>>,
    },
    /// Lift the agent's own hold ending at `until`.
    Release {
        until: DateTime<Utc>,
    },
}

impl Guard {
    /// The next call for whether a trigger holds, the action the settings name and the time.
    pub(crate) fn step(
        &mut self,
        triggered: bool,
        action: GameModeAction,
        now: DateTime<Utc>,
    ) -> Step {
        match *self {
            Self::Idle if triggered => Step::Hold { renews: None },
            Self::Idle => Step::Wait,
            // Over, or the settings page chose the other action: the old hold goes first.
            Self::Holding {
                until,
                action: held,
            } if !triggered || held != action => Step::Release { until },
            Self::Holding { until, .. } => {
                let renew_before =
                    chrono::Duration::minutes(rd_core::GAME_MODE_RENEW_BEFORE_MINUTES);
                if until - now <= renew_before {
                    Step::Hold {
                        renews: Some(until),
                    }
                } else {
                    Step::Wait
                }
            }
            Self::Yielded => {
                if !triggered {
                    *self = Self::Idle;
                }
                Step::Wait
            }
        }
    }

    /// The service answered `step`, a hold, with `answer`, made for `action`.
    pub(crate) fn held(&mut self, step: Step, answer: Held, action: GameModeAction) {
        *self = match (step, answer) {
            (Step::Hold { .. }, Held::Until(until)) => Self::Holding { until, action },
            // Somebody else's pause holds; asked again on the next look, so the game is still
            // stepped aside for once theirs ends.
            (Step::Hold { renews: None }, Held::Refused) => Self::Idle,
            (Step::Hold { renews: Some(_) }, Held::Refused) => Self::Yielded,
            _ => return,
        };
    }

    /// The service answered a release, whether anything of the agent's was still there or not.
    pub(crate) fn released(&mut self) {
        *self = Self::Idle;
    }
}

/// Starts game mode beside the agent's other tasks.
pub(crate) fn spawn(
    background: &mut tokio::task::JoinSet<()>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
    settings: watch::Receiver<CaptureAgentSettings>,
) {
    let (client, task_cancellation) = (client.clone(), cancellation.clone());
    background.spawn(supervised(
        "game mode",
        cancellation.clone(),
        notice,
        async move {
            watch_desktop(client, task_cancellation, settings).await;
            Ok(())
        },
    ));
}

/// Looks every five seconds while game mode is on, and holds while a trigger does.
async fn watch_desktop(
    client: CaptureClient,
    cancellation: CancellationToken,
    mut settings: watch::Receiver<CaptureAgentSettings>,
) {
    let mut detector = detect::Detector::default();
    let mut guard = Guard::default();
    let mut last: Option<Trigger> = None;
    let mut failing = false;
    loop {
        let mode = settings.borrow_and_update().game_mode.clone();
        let seen = if mode.active() {
            detector.look(!mode.processes.is_empty()).await
        } else {
            detector.stop();
            Seen::default()
        };
        let current = trigger(&mode, &seen);
        if current != last {
            match &current {
                Some(trigger) => tracing::info!(?trigger, "game mode: stepping aside"),
                None => tracing::info!("game mode: the desktop is free again"),
            }
            last = current.clone();
        }
        let step = guard.step(current.is_some(), mode.action, Utc::now());
        let outcome = match step {
            Step::Wait => Ok(()),
            Step::Hold { renews } => client
                .hold_game_mode(renews)
                .await
                .map(|answer| guard.held(step, answer, mode.action)),
            Step::Release { until } => client
                .release_game_mode(until)
                .await
                .map(|_| guard.released()),
        };
        // Once per outage; the step is made again on the next look.
        match outcome {
            Ok(()) => failing = false,
            Err(error) if !failing => {
                tracing::warn!(%error, ?step, "game mode: the service did not take the request");
                failing = true;
            }
            Err(_) => {}
        }
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = tokio::time::sleep(config::STATUS_POLL_INTERVAL) => {}
            // Switched off, or set otherwise: acted on now, not on the next look.
            changed = settings.changed() => if changed.is_err() { break },
        }
    }
    // The agent ends: its own hold goes with it rather than running out by itself.
    if let Guard::Holding { until, .. } = guard {
        let lifted = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.release_game_mode(until),
        )
        .await;
        if !matches!(lifted, Ok(Ok(_))) {
            tracing::info!(%until, "game mode: the hold runs out by itself");
        }
    }
}

#[cfg(test)]
#[path = "game_mode_tests.rs"]
mod tests;
