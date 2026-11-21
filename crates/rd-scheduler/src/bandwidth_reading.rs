//! The active profile's traffic budget as a notification names it (RD-1290-02): read when a
//! delivery is queued, from the counters the supervision cycle keeps, never stored again.
//!
//! The cycle books the traffic every fifteen seconds, so a package that finished between two
//! cycles would be named against the budget before it. The reading therefore adds what the queue
//! committed since the last cycle, on a copy of the counters (RD-1430-05): the next cycle books
//! the same bytes once, as it would have without the reading.

use anyhow::Result;
use rd_core::Timestamp;
use rd_limits::{BudgetReading, BudgetState};

use super::BandwidthService;

impl BandwidthService {
    /// How far the budget of the profile in force is used in the current period, with the
    /// traffic not booked yet counted in; `None` without an active profile or without a limit on
    /// it. A counter from a period that is over reads as empty, as the budget gate reads it.
    /// `database` is the one the supervision cycle samples, for the odometer it counts from.
    pub async fn budget_reading(
        &self,
        database: &rd_db::Database,
    ) -> Result<Option<BudgetReading>> {
        let limited = self
            .state
            .read()
            .await
            .active_profile()
            .is_some_and(|profile| {
                let limits = profile.budget_limits();
                limits.daily_bytes.is_some() || limits.monthly_bytes.is_some()
            });
        if !limited {
            return Ok(None);
        }
        // Read before the counters, and never with the state locked across an await. A cycle
        // that books in between leaves the copy's odometer at or past `total`, which then adds
        // nothing: what the cycle booked is in the counters already.
        let total = database.committed_bytes_total().await?;
        let baseline = super::load_baseline(database).await?;
        let state = self.state.read().await;
        let Some(profile) = state.active_profile() else {
            return Ok(None);
        };
        let now = Timestamp::now();
        // A profile the cycle has not counted yet starts where its first sample will.
        let mut budget = state
            .budgets
            .get(&profile.id.to_string())
            .cloned()
            .unwrap_or_else(|| BudgetState {
                last_total_bytes: baseline.unwrap_or(total),
                ..BudgetState::default()
            });
        let unbooked = budget.delta_from_total(total);
        budget.record(
            &state.schedule.day_key(now),
            &state.schedule.month_key(now),
            unbooked,
        );
        Ok(budget.reading(&profile.budget_limits()))
    }
}
