//! The active profile's traffic budget as a notification names it (RD-1290-02): read when a
//! delivery is queued, from the counters the supervision cycle keeps, never stored again.

use chrono::Utc;
use rd_limits::BudgetReading;

use super::BandwidthService;

impl BandwidthService {
    /// How far the budget of the profile in force is used in the current period; `None` without
    /// an active profile or without a limit on it. A counter from a period that is over reads as
    /// empty, as the budget gate reads it.
    pub async fn budget_reading(&self) -> Option<BudgetReading> {
        let state = self.state.read().await;
        let profile = state.active_profile()?;
        let now = Utc::now();
        let mut budget = state
            .budgets
            .get(&profile.id.to_string())
            .cloned()
            .unwrap_or_default();
        budget.record(
            &state.schedule.day_key(now),
            &state.schedule.month_key(now),
            0,
        );
        budget.reading(&profile.budget_limits())
    }
}
