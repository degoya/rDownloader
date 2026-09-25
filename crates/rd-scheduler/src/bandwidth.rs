//! Profile switching, live limit application and traffic budgets (RD-050-12).
//!
//! The scheduler owns the loop: it loads profiles and windows, asks the schedule which
//! profile is active, pushes the resulting limits into the shared registry and counts the
//! traffic of the current period. Everything policy-shaped lives in `rd-limits`.

use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{BandwidthProfileId, BandwidthSettings};
use rd_limits::{
    BandwidthProfile, BindingLimit, BudgetExceeded, BudgetKind, BudgetState, BudgetStates,
    LimiterRegistry, TransferScope, WeeklySchedule, parse_timezone,
};
use tokio::sync::RwLock;

use crate::SchedulerHandle;

/// Odometer of all committed bytes at the last budget sample, so a restart adds no traffic
/// that happened before it.
const BUDGET_BASELINE_KEY: &str = "bandwidth.total_baseline";

/// What the REST layer and the UI show about the current state.
#[derive(Clone, Debug)]
pub struct BandwidthStatus {
    pub active_profile: Option<BandwidthProfile>,
    pub next_switch_at: Option<DateTime<Utc>>,
    pub timezone: String,
    pub budget: Option<BudgetState>,
    pub exceeded: Option<BudgetExceeded>,
}

#[derive(Debug, Default)]
struct BandwidthState {
    schedule: WeeklySchedule,
    profiles: Vec<BandwidthProfile>,
    active: Option<BandwidthProfileId>,
    budgets: BudgetStates,
    /// Whether the *active* profile's budget is used up. New transfers wait while it is set, so
    /// it must never outlive the profile it was measured for (RD-120-64).
    exceeded: Option<BudgetExceeded>,
    /// The exhaustions already announced, as profile and period key (`2026-09-24` for a day,
    /// `2026-09` for a month). A profile that ends and comes back the same day is still used
    /// up, and saying so a second time is noise; the set forgets every period that is over.
    announced: HashSet<(BandwidthProfileId, String)>,
}

impl BandwidthState {
    /// The profile the schedule currently points at.
    fn active_profile(&self) -> Option<&BandwidthProfile> {
        let id = self.active?;
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// What the stored counters say about `profile`'s budget in the current period, without
    /// counting anything: a counter from a period that is over reads as empty.
    fn exhausted_now(
        &self,
        profile: &BandwidthProfile,
        day: &str,
        month: &str,
    ) -> Option<BudgetExceeded> {
        let mut budget = self.budgets.get(&profile.id.to_string())?.clone();
        budget.record(day, month, 0);
        budget.exhausted(&profile.budget_limits())
    }

    /// Sets the gate to `next` — what the budget of `profile`, the active one, says now — and
    /// returns the `bandwidth.changed` event to announce, if there is one.
    ///
    /// Only an edge is announced. `exhausted: true` becomes one `budget_exhausted` delivery
    /// (RD-120-62), so it goes out once per profile and period, however often the gate is
    /// cleared and set again in between — by a profile ending and returning, or by a switch
    /// back and forth. `exhausted: false` is for the interface and notifies nobody.
    fn settle_exceeded(
        &mut self,
        profile: Option<BandwidthProfileId>,
        next: Option<BudgetExceeded>,
        day: &str,
        month: &str,
    ) -> Option<rd_core::EventEnvelope> {
        let previous = std::mem::replace(&mut self.exceeded, next);
        self.announced
            .retain(|(_, period)| period == day || period == month);
        let payload = match (previous, next, profile) {
            (_, Some(next), Some(profile)) => {
                let period = match next.period {
                    BudgetKind::Daily => day,
                    BudgetKind::Monthly => month,
                };
                if !self.announced.insert((profile, period.to_owned())) {
                    return None;
                }
                serde_json::json!({
                    "entity": "budget",
                    "exhausted": true,
                    "profile": profile,
                    "period": next.period,
                    "used_bytes": next.used_bytes,
                    "limit_bytes": next.limit_bytes,
                })
            }
            (Some(_), None, profile) => serde_json::json!({
                "entity": "budget",
                "exhausted": false,
                "profile": profile,
            }),
            _ => return None,
        };
        Some(rd_core::EventEnvelope::new(
            rd_core::EventKind::BandwidthChanged,
            payload,
        ))
    }
}

/// Shared bandwidth state, cloneable like the other service handles.
#[derive(Clone, Debug, Default)]
pub struct BandwidthService {
    limits: LimiterRegistry,
    state: Arc<RwLock<BandwidthState>>,
}

impl BandwidthService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The limiter registry the transports acquire their bytes from.
    #[must_use]
    pub fn limits(&self) -> LimiterRegistry {
        self.limits.clone()
    }

    /// The strictest limit applying to a transfer, and where it comes from.
    #[must_use]
    pub fn binding_limit(&self, scope: &TransferScope) -> Option<BindingLimit> {
        self.limits.binding_limit(scope)
    }

    /// Whether the active profile's budget is used up; new transfers wait, running ones
    /// finish.
    pub async fn budget_exceeded(&self) -> Option<BudgetExceeded> {
        self.state.read().await.exceeded
    }

    pub async fn status(&self) -> BandwidthStatus {
        let state = self.state.read().await;
        let active = state.active_profile().cloned();
        BandwidthStatus {
            next_switch_at: state.schedule.next_switch_after(Utc::now()),
            timezone: state.schedule.timezone.to_string(),
            budget: active
                .as_ref()
                .and_then(|profile| state.budgets.get(&profile.id.to_string()))
                .cloned(),
            exceeded: state.exceeded,
            active_profile: active,
        }
    }

    /// The parallelism override of the active profile, if it sets one.
    pub async fn max_active_files(&self) -> Option<u32> {
        let state = self.state.read().await;
        state
            .active_profile()
            .and_then(|profile| profile.max_active_files)
    }

    /// The torrent session rates of the active profile, applied by the torrent supervisor.
    pub async fn torrent_rates(&self) -> (Option<u64>, Option<u64>) {
        let state = self.state.read().await;
        state.active_profile().map_or((None, None), |profile| {
            (
                profile
                    .download_bytes_per_second
                    .map(rd_core::ByteCount::get),
                profile.upload_bytes_per_second.map(rd_core::ByteCount::get),
            )
        })
    }
}

impl SchedulerHandle {
    /// The bandwidth policy shared with the runners and the REST layer.
    #[must_use]
    pub fn bandwidth(&self) -> BandwidthService {
        self.config.bandwidth.clone()
    }

    /// Reloads profiles, schedule and counters, then applies the active profile.
    ///
    /// Called on start, on every supervision cycle and right after a profile or schedule
    /// edit, so a change takes effect without a restart.
    pub async fn reload_bandwidth(&self) -> Result<()> {
        // Falls back to defaults rather than refusing: this runs on every supervision cycle,
        // and an unusable blob must not take the scheduler's reload loop down. Defaults mean
        // *no* limits, which is why the accessor reports the failure instead of swallowing it.
        let settings: BandwidthSettings = self.database.service_settings_or_default().await?;
        let timezone = parse_timezone(&settings.bandwidth_timezone).unwrap_or_else(|error| {
            tracing::warn!(%error, "falling back to UTC for the bandwidth schedule");
            chrono_tz::Tz::UTC
        });
        let profiles = self.database.list_bandwidth_profiles().await?;
        let windows = self.database.list_bandwidth_windows().await?;
        let mut budgets = self.database.bandwidth_budgets().await?;
        // The odometer is global, not per profile: it says how many bytes the queue had
        // already committed when the counters were last sampled.
        let baseline = self.load_budget_baseline().await.unwrap_or_default();
        for budget in budgets.values_mut() {
            budget.last_total_bytes = baseline;
        }
        let schedule = WeeklySchedule {
            timezone,
            // A default profile that was deleted must not keep applying its limits.
            default_profile_id: settings
                .bandwidth_default_profile_id
                .filter(|id| profiles.iter().any(|profile| profile.id == *id)),
            windows,
        };
        let service = self.bandwidth();
        {
            let mut state = service.state.write().await;
            state.schedule = schedule;
            state.profiles = profiles;
            state.budgets = budgets;
        }
        self.apply_active_profile().await
    }

    /// Evaluates the schedule and pushes the active profile's limits into the registry.
    pub(crate) async fn apply_active_profile(&self) -> Result<()> {
        let service = self.bandwidth();
        let (active, profile) = {
            let state = service.state.read().await;
            let active = state.schedule.active_at(Utc::now());
            let profile = active
                .and_then(|id| state.profiles.iter().find(|profile| profile.id == id))
                .cloned();
            (active, profile)
        };
        let (switched, budget_event) = {
            let mut state = service.state.write().await;
            let switched = state.active != active;
            state.active = active;
            // The gate follows the profile that is active *now* (RD-120-64). Left to the next
            // budget sample, a used-up budget outlived its profile: with no profile active the
            // sample returns early and never cleared it, and after a switch the old profile's
            // verdict held new transfers back until the next tick measured the new one.
            let now = Utc::now();
            let (day, month) = (state.schedule.day_key(now), state.schedule.month_key(now));
            let next = profile
                .as_ref()
                .and_then(|profile| state.exhausted_now(profile, &day, &month));
            let event = state.settle_exceeded(active, next, &day, &month);
            (switched, event)
        };
        if let Some(event) = budget_event {
            self.database.broadcast(event);
        }
        match &profile {
            Some(profile) => service.limits.apply(
                profile
                    .download_bytes_per_second
                    .map(rd_core::ByteCount::get),
                &profile.scope_limits(),
            ),
            None => service.limits.apply(None, &[]),
        }
        if switched {
            let name = profile
                .as_ref()
                .map_or("none", |profile| profile.name.as_str());
            tracing::info!(profile = name, "bandwidth profile switched");
            self.database.broadcast(rd_core::EventEnvelope::new(
                rd_core::EventKind::BandwidthChanged,
                serde_json::json!({ "entity": "active", "profile": profile.as_ref().map(|p| p.id) }),
            ));
        }
        Ok(())
    }

    /// Adds the traffic since the last sample to the active profile's counters.
    pub(crate) async fn record_bandwidth_budget(&self) -> Result<()> {
        let service = self.bandwidth();
        let Some(profile) = ({
            let state = service.state.read().await;
            state.active_profile().cloned()
        }) else {
            // Without an active profile there is no budget; the odometer still advances so
            // the next profile does not inherit that gap as traffic of its own.
            let total = self.committed_total().await?;
            if self.load_budget_baseline().await != Some(total) {
                self.store_budget_baseline(total).await?;
            }
            return Ok(());
        };
        let now = Utc::now();
        let (day, month) = {
            let state = service.state.read().await;
            (state.schedule.day_key(now), state.schedule.month_key(now))
        };
        let total = self.committed_total().await?;
        let baseline = self.load_budget_baseline().await;
        let key = profile.id.to_string();
        let (state_to_store, exceeded, advanced, budget_event) = {
            let mut state = service.state.write().await;
            // A profile counting for the first time starts from the current odometer, so it
            // is not charged for everything downloaded before it existed.
            let budget = state.budgets.entry(key).or_insert_with(|| BudgetState {
                last_total_bytes: baseline.unwrap_or(total),
                ..BudgetState::default()
            });
            let delta = budget.delta_from_total(total);
            budget.record(&day, &month, delta);
            let exceeded = budget.exhausted(&profile.budget_limits());
            let snapshot = budget.clone();
            let event = state.settle_exceeded(Some(profile.id), exceeded, &day, &month);
            (snapshot, exceeded, delta > 0, event)
        };
        // Announced on the transition, before the early return below: the budget coming back
        // at a new period moves no bytes, and the UI still has to hear about it.
        if let Some(event) = budget_event {
            self.database.broadcast(event);
        }
        // A tick that moved nothing writes nothing; the period key alone is not worth a
        // database round trip every fifteen seconds.
        if !advanced {
            return Ok(());
        }
        // The budget first, the baseline second. These are two writes and either can fail on
        // its own; the order decides which way the error falls. Written this way, a failure
        // leaves the baseline behind the odometer and the next tick counts the same traffic
        // again — a budget that reads slightly high. The other way round the odometer has
        // already moved past bytes no budget ever saw, and nothing later can find them: the
        // traffic is invisible to the daily and the monthly limit for good.
        self.database
            .store_bandwidth_budget(profile.id, state_to_store)
            .await?;
        self.store_budget_baseline(total).await?;
        if let Some(exceeded) = exceeded {
            tracing::debug!(
                used = exceeded.used_bytes,
                limit = exceeded.limit_bytes,
                "traffic budget exhausted"
            );
        }
        Ok(())
    }

    /// Total committed bytes across the queue — the odometer the budget deltas come from.
    ///
    /// Using the persisted progress rather than the limiters covers every transport,
    /// including the ones that run as external processes and never pass through a bucket.
    async fn committed_total(&self) -> Result<u64> {
        Ok(self
            .database
            .list_downloads()
            .await?
            .into_iter()
            .fold(0u64, |total, file| {
                total.saturating_add(file.committed_bytes.get())
            }))
    }

    async fn load_budget_baseline(&self) -> Option<u64> {
        self.database
            .get_setting(BUDGET_BASELINE_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|value| value.as_u64())
    }

    async fn store_budget_baseline(&self, total: u64) -> Result<()> {
        self.database
            .set_setting(BUDGET_BASELINE_KEY.to_owned(), serde_json::json!(total))
            .await
    }
}

impl SchedulerHandle {
    /// The scope chain a file's transfer is limited by: its transport, host, account and
    /// the category its package is routed into.
    pub(crate) async fn transfer_scope(&self, file: &rd_core::DownloadFile) -> TransferScope {
        let category_id = self
            .database
            .list_packages()
            .await
            .ok()
            .and_then(|packages| {
                packages
                    .into_iter()
                    .find(|package| package.id == file.package_id)
            })
            .and_then(|package| package.category_id);
        TransferScope::for_download(
            file.kind,
            file.source.host_str(),
            file.account_id,
            category_id,
        )
    }

    /// A limiter bound to one file's scope, handed to the transport that runs it.
    pub(crate) async fn scoped_limiter(
        &self,
        file: &rd_core::DownloadFile,
    ) -> rd_limits::ScopedLimiter {
        let scope = self.transfer_scope(file).await;
        self.config.bandwidth.limits().scoped(scope)
    }

    /// One supervision cycle: reload, re-evaluate the schedule, count the traffic.
    pub(crate) async fn supervise_bandwidth(&self) -> Result<()> {
        self.reload_bandwidth().await?;
        self.record_bandwidth_budget().await
    }
}

#[cfg(test)]
#[path = "bandwidth_tests.rs"]
mod tests;
