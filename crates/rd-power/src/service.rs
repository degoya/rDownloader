//! The completion state machine: it decides when the queue has finished a cycle of work,
//! runs the configured action exactly once for it, and lets a person cancel a power action
//! while the countdown runs.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::{
    adapter::{PowerAdapter, PowerCapabilities, PowerState},
    settings::{CompletionAction, PowerSettings},
};

/// A power action waiting out its countdown.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PendingAction {
    pub action: CompletionAction,
    pub runs_at: DateTime<Utc>,
    /// The work cycle this action belongs to; a restart mid-countdown resumes the same one.
    pub cycle: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PowerStatus {
    pub capabilities: PowerCapabilities,
    pub state: PowerState,
    pub action: CompletionAction,
    /// Set while a power action is configured but has no local approval, so the UI can say
    /// why nothing will happen.
    pub approval_missing: bool,
    pub pending: Option<PendingAction>,
    pub quiet: bool,
    pub quiet_until: Option<DateTime<Utc>>,
    /// Why the queue is paused by the network context, if it is.
    pub paused_reason: Option<&'static str>,
    /// Whether the machine is currently being kept awake.
    pub inhibiting: bool,
}

#[derive(Debug)]
struct ServiceState {
    settings: PowerSettings,
    timezone: Tz,
    state: PowerState,
    /// Increments whenever the queue goes from idle to busy; the action fires once per
    /// cycle, which is what makes it exactly-once across restarts and queue flicker.
    cycle: u64,
    /// The last cycle whose completion action already fired.
    completed_cycle: u64,
    busy: bool,
    pending: Option<PendingAction>,
    /// Held while the machine is being kept awake; dropping it releases the inhibition.
    inhibition: Option<crate::adapter::Inhibition>,
    /// Whether the held inhibition covers the display, so a changed setting re-acquires.
    inhibits_display: bool,
    /// Set after a failed attempt, so a platform that cannot do this is not asked again every
    /// tick. Cleared when the queue goes idle, which is the next honest moment to retry.
    inhibit_failed: bool,
    /// Bumped every time the inhibition wish changes. Taking an inhibition means spawning a
    /// platform helper, which happens with the lock released; this is what tells the caller
    /// coming back whether the wish it acted on is still the current one, or whether the
    /// queue went idle — or the display setting moved — while the helper was starting.
    inhibit_generation: u64,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            settings: PowerSettings::default(),
            timezone: Tz::UTC,
            state: PowerState::default(),
            cycle: 0,
            completed_cycle: 0,
            busy: false,
            pending: None,
            inhibition: None,
            inhibits_display: false,
            inhibit_failed: false,
            inhibit_generation: 0,
        }
    }
}

/// Cloneable handle shared by the scheduler loop and the REST layer.
#[derive(Clone)]
pub struct PowerService {
    adapter: Arc<dyn PowerAdapter>,
    state: Arc<RwLock<ServiceState>>,
}

impl std::fmt::Debug for PowerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PowerService")
            .finish_non_exhaustive()
    }
}

impl Default for PowerService {
    fn default() -> Self {
        Self::new(crate::adapter::platform_adapter())
    }
}

impl PowerService {
    #[must_use]
    pub fn new(adapter: Arc<dyn PowerAdapter>) -> Self {
        Self {
            adapter,
            state: Arc::new(RwLock::new(ServiceState::default())),
        }
    }

    /// Applies the settings and the schedule timezone; called on start and on every save.
    pub async fn apply(&self, settings: PowerSettings, timezone: Tz) {
        let mut state = self.state.write().await;
        // Turning the action off or withdrawing the approval cancels a running countdown:
        // the user just said they do not want it.
        let keeps_pending = settings.completion_action != CompletionAction::None
            && (!settings.completion_action.is_destructive() || settings.power_actions_allowed);
        if !keeps_pending {
            state.pending = None;
        }
        state.settings = settings;
        state.timezone = timezone;
    }

    /// Restores the cycle counters after a restart, so a completed cycle stays completed.
    pub async fn restore(&self, cycle: u64, completed_cycle: u64) {
        let mut state = self.state.write().await;
        state.cycle = cycle;
        state.completed_cycle = completed_cycle;
    }

    /// The cycle counters, for persisting them.
    pub async fn cycles(&self) -> (u64, u64) {
        let state = self.state.read().await;
        (state.cycle, state.completed_cycle)
    }

    /// Re-reads the platform context. Cheap enough for the supervision tick.
    pub async fn refresh_state(&self) {
        let state = self.adapter.state().await;
        self.state.write().await.state = state;
    }

    /// Whether the network context asks for the queue to hold, and why.
    pub async fn hold_reason(&self) -> Option<&'static str> {
        let state = self.state.read().await;
        if state.settings.pause_on_battery && state.state.on_battery == Some(true) {
            return Some("battery");
        }
        if state.settings.pause_on_metered && state.state.metered == Some(true) {
            return Some("metered");
        }
        None
    }

    /// Whether resource-intensive work should wait right now.
    pub async fn is_quiet(&self, now: DateTime<Utc>) -> bool {
        let state = self.state.read().await;
        state.settings.quiet_hours.is_quiet(state.timezone, now)
    }

    /// Whether post-processing should be deferred right now.
    pub async fn defers_postprocess(&self, now: DateTime<Utc>) -> bool {
        let state = self.state.read().await;
        state.settings.quiet_hours_defer_postprocess
            && state.settings.quiet_hours.is_quiet(state.timezone, now)
    }

    /// Whether notification delivery should be grouped until the quiet period ends.
    pub async fn defers_notifications(&self, now: DateTime<Utc>) -> bool {
        let state = self.state.read().await;
        state.settings.quiet_hours_defer_notifications
            && state.settings.quiet_hours.is_quiet(state.timezone, now)
    }

    /// Feeds the queue's busy state in and returns an action that is now due.
    ///
    /// `busy` covers the queue *and* post-processing: an action must not fire while a
    /// package is still being unpacked.
    pub async fn observe(&self, busy: bool, now: DateTime<Utc>) -> Option<PendingAction> {
        let mut state = self.state.write().await;
        if busy {
            if !state.busy {
                state.busy = true;
                state.cycle = state.cycle.saturating_add(1);
            }
            // New work cancels a countdown that was started for the cycle before it.
            state.pending = None;
            return None;
        }
        state.busy = false;
        if state.cycle == 0 || state.completed_cycle >= state.cycle {
            return None;
        }
        let action = state.settings.completion_action;
        if action == CompletionAction::None {
            state.completed_cycle = state.cycle;
            return None;
        }
        if action.is_destructive() && !state.settings.power_actions_allowed {
            // Configured but not approved on this machine: say nothing happens rather than
            // running it, and do not keep re-evaluating the same cycle.
            state.completed_cycle = state.cycle;
            tracing::info!(?action, "completion action skipped: no local approval");
            return None;
        }
        if let Some(pending) = state.pending.clone() {
            if pending.runs_at > now {
                return None;
            }
            state.completed_cycle = state.cycle;
            state.pending = None;
            return Some(pending);
        }
        if !action.is_destructive() {
            // A script needs no countdown; nothing to cancel and nothing to lose.
            state.completed_cycle = state.cycle;
            return Some(PendingAction {
                action,
                runs_at: now,
                cycle: state.cycle,
            });
        }
        let countdown = chrono::Duration::seconds(i64::from(
            state.settings.completion_countdown_seconds.max(1),
        ));
        state.pending = Some(PendingAction {
            action,
            runs_at: now + countdown,
            cycle: state.cycle,
        });
        None
    }

    /// Cancels a running countdown. The cycle counts as handled, so the action does not
    /// start again a second later.
    pub async fn cancel(&self) -> bool {
        let mut state = self.state.write().await;
        if state.pending.take().is_none() {
            return false;
        }
        state.completed_cycle = state.cycle;
        true
    }

    /// Runs a power action through the platform adapter.
    pub async fn execute(&self, action: CompletionAction) -> anyhow::Result<()> {
        match action {
            CompletionAction::Standby => self.adapter.standby().await,
            CompletionAction::Shutdown => self.adapter.shutdown().await,
            CompletionAction::None | CompletionAction::Script => Ok(()),
        }
    }

    pub async fn status(&self, now: DateTime<Utc>) -> PowerStatus {
        let state = self.state.read().await;
        let action = state.settings.completion_action;
        PowerStatus {
            capabilities: self.adapter.capabilities(),
            state: state.state,
            action,
            approval_missing: action.is_destructive() && !state.settings.power_actions_allowed,
            pending: state.pending.clone(),
            quiet: state.settings.quiet_hours.is_quiet(state.timezone, now),
            quiet_until: state.settings.quiet_hours.ends_after(state.timezone, now),
            paused_reason: if state.settings.pause_on_battery
                && state.state.on_battery == Some(true)
            {
                Some("battery")
            } else if state.settings.pause_on_metered && state.state.metered == Some(true) {
                Some("metered")
            } else {
                None
            },
            inhibiting: state.inhibition.is_some(),
        }
    }

    /// Keeps the machine awake while `working`, and lets it sleep again when it stops.
    ///
    /// Idempotent: holding an inhibition is the steady state during a download, so the common
    /// call does nothing at all. The guard is re-acquired when the display wish changes,
    /// because the platform decides that when the inhibition is taken, not afterwards.
    ///
    /// Taking an inhibition spawns a platform helper process, and that happens with the lock
    /// released. Holding the write guard across it parked every reader — `status`,
    /// `hold_reason`, `is_quiet`, `defers_postprocess`, called from the REST layer and from
    /// the supervision tick — behind a process spawn. `refresh_state` has the same shape:
    /// probe outside the lock, store inside.
    pub async fn set_inhibited(&self, working: bool) {
        let Some((display, generation)) = ({
            let mut state = self.state.write().await;
            let wanted = working && state.settings.prevent_standby;
            let display = state.settings.prevent_display_standby;
            if !wanted {
                // Dropping the guard is what releases it. The bumped generation also
                // disowns an acquisition still in flight, so a helper that finishes
                // starting after the queue went idle does not keep the machine awake.
                state.inhibition = None;
                state.inhibit_failed = false;
                state.inhibit_generation = state.inhibit_generation.wrapping_add(1);
                None
            } else if state.inhibition.is_some() && state.inhibits_display == display {
                None
            } else if state.inhibit_failed && state.inhibits_display == display {
                // A platform that cannot do this would otherwise be asked, and warn, every
                // tick.
                None
            } else {
                state.inhibition = None;
                state.inhibits_display = display;
                state.inhibit_generation = state.inhibit_generation.wrapping_add(1);
                Some((display, state.inhibit_generation))
            }
        }) else {
            return;
        };
        let acquired = self.adapter.inhibit(display).await;
        let mut state = self.state.write().await;
        if state.inhibit_generation != generation {
            // The wish changed while the helper was starting: the queue went idle, or the
            // display setting moved, and a later call has already acted on it. That decision
            // is the newer one, so this result is discarded rather than written over it —
            // dropping `acquired` releases whatever the platform handed out.
            return;
        }
        match acquired {
            Ok(inhibition) => {
                state.inhibition = Some(inhibition);
                state.inhibit_failed = false;
            }
            Err(error) => {
                state.inhibit_failed = true;
                tracing::warn!(%error, "the machine could not be kept awake");
            }
        }
    }

    /// The script the completion action should run, if one is configured.
    pub async fn completion_script(&self) -> Option<String> {
        let state = self.state.read().await;
        (state.settings.completion_action == CompletionAction::Script)
            .then(|| state.settings.completion_script.clone())
            .flatten()
    }
}
