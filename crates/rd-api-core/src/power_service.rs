//! Supervises the queue completion cycle and the network context (RD-050-13).
//!
//! Lives here rather than in the scheduler because it needs both sides: the queue tells it
//! when work is done, and the extraction service owns the script sandbox the completion
//! action reuses.

use std::{sync::Arc, time::Duration};

use rd_power::CompletionAction;
use tokio_util::sync::CancellationToken;

/// How often the queue state and the platform context are sampled.
const TICK: Duration = Duration::from_secs(5);

/// Persisted cycle counters, so a completed cycle stays completed across a restart.
const CYCLE_KEY: &str = "power.completion_cycle";

struct Inner {
    database: rd_db::Database,
    scheduler: rd_scheduler::SchedulerHandle,
    extraction: rd_extract::ExtractionService,
    power: rd_power::PowerService,
    quiet_hold: QuietHold,
    /// The countdown the last `power.changed` announced (RD-1260-04).
    countdown: std::sync::Mutex<CountdownNotice>,
    shutdown: CancellationToken,
}

/// Cloneable handle of the background completion loop.
#[derive(Clone)]
pub struct PowerSupervisor {
    inner: Arc<Inner>,
}

impl PowerSupervisor {
    #[must_use]
    pub fn start(
        database: rd_db::Database,
        scheduler: rd_scheduler::SchedulerHandle,
        extraction: rd_extract::ExtractionService,
        power: rd_power::PowerService,
        quiet_hold: QuietHold,
    ) -> Self {
        let supervisor = Self {
            inner: Arc::new(Inner {
                database,
                scheduler,
                extraction,
                power,
                quiet_hold,
                countdown: std::sync::Mutex::default(),
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(supervisor.clone().run());
        supervisor
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    async fn run(self) {
        if let Err(error) = self.restore().await {
            tracing::warn!(%error, "completion cycle state could not be restored");
        }
        let mut ticker = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.tick().await {
                        tracing::warn!(%error, "power supervision failed");
                    }
                }
            }
        }
    }

    async fn restore(&self) -> anyhow::Result<()> {
        let stored = self.inner.database.get_setting(CYCLE_KEY).await?;
        if let Some(stored) = stored {
            let cycle = stored["cycle"].as_u64().unwrap_or_default();
            let completed = stored["completed"].as_u64().unwrap_or_default();
            self.inner.power.restore(cycle, completed).await;
        }
        Ok(())
    }

    /// Re-reads the power settings from the shared blob, so a change applies without a
    /// restart and a direct write is picked up the same way the other services do it.
    async fn reload_settings(&self) -> anyhow::Result<()> {
        // Falls back to defaults rather than refusing: this runs on every tick and must keep
        // the quiet-hours and network holds supervised. The blob is read once and both slices
        // come out of the same value, so a tick sees one consistent configuration.
        let Some(blob) = self
            .inner
            .database
            .get_setting(rd_db::SERVICE_SETTINGS_KEY)
            .await?
        else {
            return Ok(());
        };
        let settings: rd_power::PowerSettings =
            rd_db::parse_service_settings(&blob).unwrap_or_default();
        let timezone = rd_db::service_setting_field_of::<String>(&blob, "bandwidth_timezone")
            .and_then(|value| rd_limits::parse_timezone(&value).ok())
            .unwrap_or_else(rd_limits::default_timezone);
        self.inner.power.apply(settings, timezone).await;
        Ok(())
    }

    async fn tick(&self) -> anyhow::Result<()> {
        self.reload_settings().await?;
        let power = &self.inner.power;
        power.refresh_state().await;
        self.apply_quiet_hold().await;
        self.apply_network_hold().await;

        let downloads = self.inner.database.list_downloads().await?;
        let unpacking = !self.inner.extraction.pending().await.is_empty();
        power
            .set_inhibited(unpacking || is_working(&downloads))
            .await;

        let now = chrono::Utc::now();
        let busy = unpacking || is_busy(&downloads);
        let (before_cycle, before_completed) = power.cycles().await;
        let due = power.observe(busy, now).await;
        let (cycle, completed) = power.cycles().await;
        if (cycle, completed) != (before_cycle, before_completed) {
            self.inner
                .database
                .set_setting(
                    CYCLE_KEY.to_owned(),
                    serde_json::json!({ "cycle": cycle, "completed": completed }),
                )
                .await?;
        }
        let pending = power.status(now).await.pending;
        // A countdown is announced when it starts and when it ends, not on every tick: each
        // tick's event used to be one more "power action pending" notification (RD-1260-04).
        let change = self
            .inner
            .countdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(pending.as_ref(), due.is_some(), busy);
        if let Some(payload) = change {
            self.inner.database.broadcast(rd_core::EventEnvelope::new(
                rd_core::EventKind::PowerChanged,
                payload,
            ));
        }
        if let Some(due) = due {
            self.execute(due.action).await;
        }
        Ok(())
    }

    /// Post-processing waits while quiet hours defer it; the hold is the same mechanism the
    /// pipeline already uses to pause downloads.
    async fn apply_quiet_hold(&self) {
        let quiet = self
            .inner
            .power
            .defers_postprocess(chrono::Utc::now())
            .await;
        self.inner.quiet_hold.set(quiet);
    }

    /// Battery or metered operation holds the queue, with the reason visible in the status.
    async fn apply_network_hold(&self) {
        let hold = self.inner.power.hold_reason().await;
        self.inner
            .scheduler
            .set_network_hold(rd_scheduler::HoldSource::Power, hold)
            .await;
    }

    async fn execute(&self, action: CompletionAction) {
        match action {
            CompletionAction::Script => {
                let Some(script) = self.inner.power.completion_script().await else {
                    return;
                };
                match self.inner.extraction.run_completion_script(&script).await {
                    Ok(true) => tracing::info!(script, "queue completion script finished"),
                    Ok(false) => tracing::warn!(script, "queue completion script failed"),
                    Err(error) => tracing::warn!(script, %error, "queue completion script failed"),
                }
            }
            CompletionAction::Standby | CompletionAction::Shutdown => {
                tracing::info!(?action, "running queue completion power action");
                if let Err(error) = self.inner.power.execute(action).await {
                    tracing::error!(?action, %error, "power action failed");
                }
            }
            CompletionAction::None => {}
        }
    }
}

/// The countdown last announced, so a countdown is one `power.changed` when it starts and one
/// when it ends (RD-1260-04) -- the model is the budget gate's edge (`rd_scheduler`'s
/// `settle_exceeded`).
#[derive(Debug, Default)]
struct CountdownNotice {
    announced: Option<rd_power::PendingAction>,
}

impl CountdownNotice {
    /// The `power.changed` payload a tick that saw `pending` announces, if any. `ran`: the tick
    /// runs the action; `busy`: new work arrived, which calls a countdown off.
    fn observe(
        &mut self,
        pending: Option<&rd_power::PendingAction>,
        ran: bool,
        busy: bool,
    ) -> Option<serde_json::Value> {
        let before = self.announced.take();
        self.announced = pending.cloned();
        match (before, pending) {
            (Some(before), Some(now))
                if before.cycle == now.cycle && before.runs_at == now.runs_at =>
            {
                None
            }
            (_, Some(now)) => Some(serde_json::json!({
                "state": "pending",
                "pending": now.action,
                "runs_at": now.runs_at,
            })),
            (Some(before), None) if ran => Some(serde_json::json!({
                "state": "running",
                "action": before.action,
            })),
            (Some(before), None) => Some(serde_json::json!({
                "state": "cancelled",
                "action": before.action,
                "reason": if busy { "new_work" } else { "cancelled" },
            })),
            (None, None) => None,
        }
    }
}

/// A `PostprocessHold` that is raised and lowered as a flag rather than per job.
#[derive(Clone)]
pub struct QuietHold {
    hold: rd_core::PostprocessHold,
    guard: Arc<std::sync::Mutex<Option<rd_core::PostprocessHoldGuard>>>,
}

impl QuietHold {
    #[must_use]
    pub fn new(hold: rd_core::PostprocessHold) -> Self {
        Self {
            hold,
            guard: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Raises or lowers the hold; repeated calls with the same value do nothing.
    pub fn set(&self, held: bool) {
        let mut guard = self.guard.lock().unwrap_or_else(|error| error.into_inner());
        match (held, guard.is_some()) {
            (true, false) => *guard = Some(self.hold.acquire()),
            (false, true) => *guard = None,
            _ => {}
        }
    }
}

impl std::fmt::Debug for QuietHold {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("QuietHold").finish_non_exhaustive()
    }
}

/// Work is in flight while any download is queued or running.
///
/// Used for the completion cycle, which must not fire between a download and its unpacking, so
/// a file merely waiting its turn still counts.
fn is_busy(downloads: &[rd_core::DownloadFile]) -> bool {
    downloads
        .iter()
        .any(|file| file.state.is_queued_or_working())
}

/// Whether something is actually being worked on right now.
///
/// Deliberately narrower than [`is_busy`]: a queue held back by a bandwidth schedule, or a file
/// waiting out an IP block, is not a reason to keep a machine awake for hours. Those states
/// still stop the completion action, which is a different question.
fn is_working(downloads: &[rd_core::DownloadFile]) -> bool {
    downloads.iter().any(|file| file.state.is_working())
}

#[cfg(test)]
#[path = "power_service_tests.rs"]
mod tests;
