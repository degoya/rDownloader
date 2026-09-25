//! Asking the router for a new address when free downloads are stuck behind an IP limit
//! (RD-094-04).
//!
//! Hosters that limit anonymous downloads do so per address, so the only way past is a
//! different address. What that takes is specific to the router, which is why this runs a
//! script the operator writes rather than trying to speak to the router itself — the same
//! sandbox post-processing scripts already run in.
//!
//! The attempt is deliberately conservative. It holds the queue rather than racing it, it
//! never interrupts a running transfer unless it was told it may, and it keeps its distance
//! from the previous attempt: reconnecting in a loop against a hoster that is simply refusing
//! achieves nothing and looks like abuse from the other end.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rd_core::DownloadState;
use serde::Serialize;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::{
    AppState,
    reconnect_decision::{ReconnectInputs, ReconnectVerdict, evaluate},
};

/// Checked every fifteen seconds. An IP block lasts minutes at least, so there is nothing to
/// gain from looking more often, and the check costs one settings read.
const TICK: Duration = Duration::from_secs(15);

/// Where the last attempt got to, so the interface can say what is happening.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconnectPhase {
    #[default]
    Idle,
    /// The queue is held and running transfers are being let go of.
    Draining,
    /// The script is running.
    Running,
    /// The script is done; waiting for the address to actually change.
    WaitingForAddress,
}

/// What happened last time.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ReconnectAttempt {
    pub at: DateTime<Utc>,
    pub success: bool,
    pub old_address: Option<String>,
    pub new_address: Option<String>,
    pub error: Option<String>,
}

/// The reconnect state, as the interface sees it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ReconnectStatus {
    pub enabled: bool,
    pub phase: ReconnectPhase,
    pub last: Option<ReconnectAttempt>,
    /// Earliest time another attempt may run.
    pub next_allowed_at: Option<DateTime<Utc>>,
    /// Hosters currently held back by an address limit.
    pub blocked_hosts: Vec<BlockedHost>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BlockedHost {
    pub host: String,
    pub until: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct State {
    phase: ReconnectPhase,
    last: Option<ReconnectAttempt>,
    last_attempt_at: Option<DateTime<Utc>>,
}

/// Cloneable handle of the reconnect loop.
#[derive(Clone, Debug, Default)]
pub struct ReconnectService {
    state: Arc<RwLock<State>>,
}

impl ReconnectService {
    /// Starts the watcher. Ends with the application state, like the other loops here.
    pub fn start(&self, app: AppState) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            loop {
                ticker.tick().await;
                if let Err(error) = service.tick(&app).await {
                    tracing::warn!(%error, "the reconnect watcher failed");
                }
            }
        });
    }

    async fn tick(&self, app: &AppState) -> anyhow::Result<()> {
        // Never two at once: the second would hold a queue the first already released.
        if self.state.read().await.phase != ReconnectPhase::Idle {
            return Ok(());
        }
        let settings = match crate::handlers::stored_settings(&app.database).await {
            Ok(settings) => settings,
            Err(error) => anyhow::bail!("{}", error.message()),
        };
        if !settings.reconnect_enabled {
            return Ok(());
        }
        let downloads = app.database.list_downloads().await?;
        let timezone = rd_limits::parse_timezone(&settings.bandwidth_timezone)
            .unwrap_or_else(|_| rd_limits::default_timezone());
        let inputs = ReconnectInputs {
            enabled: settings.reconnect_enabled,
            has_script: settings
                .reconnect_script
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty()),
            windows: &settings.reconnect_windows,
            timezone,
            min_interval_minutes: settings.reconnect_min_interval_minutes,
            last_attempt: self.state.read().await.last_attempt_at,
            abort_active: settings.reconnect_abort_active,
            now: Utc::now(),
        };
        if evaluate(&inputs, &downloads) != ReconnectVerdict::Go {
            return Ok(());
        }
        self.run(app, &settings).await;
        Ok(())
    }

    /// Runs one attempt, whatever the outcome. Also the manual trigger's entry point.
    pub(crate) async fn run(&self, app: &AppState, settings: &crate::dto::SettingsResponse) {
        {
            let mut state = self.state.write().await;
            state.phase = ReconnectPhase::Draining;
            state.last_attempt_at = Some(Utc::now());
        }
        let timeout = Duration::from_secs(u64::from(settings.reconnect_timeout_seconds));
        // The whole attempt is bounded, not just the script: a router that comes back without
        // a new address would otherwise hold the queue indefinitely.
        let outcome = tokio::time::timeout(timeout, self.attempt(app, settings))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the reconnect did not finish in time")));

        // Whatever happened, the queue is released and anything paused for this is resumed.
        let paused = self.release(app).await;
        let attempt = match outcome {
            Ok((old, new)) => {
                app.scheduler.clear_host_blocks();
                let requeued = app.scheduler.requeue_ip_blocked().await.unwrap_or_default();
                tracing::info!(?old, ?new, requeued, "reconnected");
                ReconnectAttempt {
                    at: Utc::now(),
                    success: true,
                    old_address: old,
                    new_address: new,
                    error: None,
                }
            }
            Err(error) => {
                let error = format!("{error:#}");
                tracing::warn!(%error, "the reconnect failed");
                ReconnectAttempt {
                    at: Utc::now(),
                    success: false,
                    old_address: None,
                    new_address: None,
                    error: Some(error),
                }
            }
        };
        let _ = paused;
        {
            let mut state = self.state.write().await;
            state.phase = ReconnectPhase::Idle;
            state.last = Some(attempt.clone());
        }
        broadcast(app, &attempt).await;
    }

    /// The attempt itself: note the address, run the script, wait for it to change.
    async fn attempt(
        &self,
        app: &AppState,
        settings: &crate::dto::SettingsResponse,
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        app.scheduler
            .set_network_hold(rd_scheduler::HoldSource::Reconnect, Some("reconnect"))
            .await;
        self.pause_transfers(app, settings).await?;

        let before = crate::reconnect_ip::public_address(&settings.reconnect_ip_check_urls).await;
        self.set_phase(ReconnectPhase::Running).await;
        let name = settings
            .reconnect_script
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("no reconnect script is configured"))?;
        let ok = app
            .extraction
            .run_named_script(
                name,
                &rd_extract::StandaloneScript {
                    kind: "reconnect".to_owned(),
                    package_name: "reconnect".to_owned(),
                    ..rd_extract::StandaloneScript::default()
                },
            )
            .await?;
        anyhow::ensure!(ok, "the reconnect script reported failure");

        self.set_phase(ReconnectPhase::WaitingForAddress).await;
        let after = crate::reconnect_ip::wait_for_change(
            &settings.reconnect_ip_check_urls,
            before.as_deref(),
        )
        .await;
        Ok((before, after))
    }

    /// Pauses what a dropped connection would otherwise break, and reports what was paused.
    async fn pause_transfers(
        &self,
        app: &AppState,
        settings: &crate::dto::SettingsResponse,
    ) -> anyhow::Result<()> {
        if !settings.reconnect_abort_active {
            return Ok(());
        }
        for file in app.database.list_downloads().await? {
            if matches!(
                file.state,
                DownloadState::Downloading | DownloadState::Resolving
            ) {
                // Paused rather than cancelled: a resumable transfer picks up where it left off.
                let _ = app.scheduler.pause(file.id).await;
            }
        }
        Ok(())
    }

    /// Releases the hold and resumes what was paused for the attempt.
    async fn release(&self, app: &AppState) -> usize {
        app.scheduler
            .set_network_hold(rd_scheduler::HoldSource::Reconnect, None)
            .await;
        let Ok(downloads) = app.database.list_downloads().await else {
            return 0;
        };
        let mut resumed = 0;
        for file in downloads {
            if file.state == DownloadState::Paused && app.scheduler.resume(file.id).await.is_ok() {
                resumed += 1;
            }
        }
        resumed
    }

    async fn set_phase(&self, phase: ReconnectPhase) {
        self.state.write().await.phase = phase;
    }

    /// The state the interface reads.
    pub(crate) async fn status(
        &self,
        app: &AppState,
        settings: &crate::dto::SettingsResponse,
    ) -> ReconnectStatus {
        let state = self.state.read().await;
        let next_allowed_at = state.last_attempt_at.map(|last| {
            last + chrono::Duration::minutes(i64::from(settings.reconnect_min_interval_minutes))
        });
        ReconnectStatus {
            enabled: settings.reconnect_enabled,
            phase: state.phase,
            last: state.last.clone(),
            next_allowed_at,
            blocked_hosts: app
                .scheduler
                .blocked_hosts()
                .into_iter()
                .map(|(host, until)| BlockedHost { host, until })
                .collect(),
        }
    }

    /// Whether an attempt is in flight, so the manual trigger can refuse a second one.
    pub(crate) async fn busy(&self) -> bool {
        self.state.read().await.phase != ReconnectPhase::Idle
    }
}

async fn broadcast(app: &AppState, attempt: &ReconnectAttempt) {
    app.database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::ReconnectChanged,
        serde_json::json!({
            "success": attempt.success,
            "old_address": attempt.old_address,
            "new_address": attempt.new_address,
            "error": attempt.error,
        }),
    ));
}
