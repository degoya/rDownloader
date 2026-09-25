//! Daily and monthly traffic budgets.
//!
//! Counters are keyed by the period they belong to (`2026-09-03`, `2026-09`) rather than by
//! a rolling window. A restart therefore resumes the same counter instead of starting a new
//! one, and a DST change can neither create a second period nor swallow one, because the
//! key comes from the local calendar date.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One period's byte counter.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetPeriod {
    pub key: String,
    pub used_bytes: u64,
}

/// The persisted state of both counters.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct BudgetState {
    pub daily: BudgetPeriod,
    pub monthly: BudgetPeriod,
    /// Total committed bytes at the last sample, so a restart adds no phantom traffic.
    pub last_total_bytes: u64,
}

impl BudgetState {
    /// Rolls the counters onto `day`/`month` and adds `bytes`.
    ///
    /// A period change resets that counter to the new period before the bytes land, so
    /// traffic is never attributed to the period it did not happen in.
    pub fn record(&mut self, day: &str, month: &str, bytes: u64) {
        if self.daily.key != day {
            self.daily = BudgetPeriod {
                key: day.to_owned(),
                used_bytes: 0,
            };
        }
        if self.monthly.key != month {
            self.monthly = BudgetPeriod {
                key: month.to_owned(),
                used_bytes: 0,
            };
        }
        self.daily.used_bytes = self.daily.used_bytes.saturating_add(bytes);
        self.monthly.used_bytes = self.monthly.used_bytes.saturating_add(bytes);
    }

    /// Bytes to add from a new total, ignoring a total that shrank because downloads were
    /// removed — deleted history is not negative traffic.
    #[must_use]
    pub fn delta_from_total(&mut self, total_bytes: u64) -> u64 {
        let delta = total_bytes.saturating_sub(self.last_total_bytes);
        self.last_total_bytes = total_bytes;
        delta
    }

    /// Which budgets are exhausted right now.
    #[must_use]
    pub fn exhausted(&self, limits: &BudgetLimits) -> Option<BudgetExceeded> {
        if let Some(limit) = limits.daily_bytes
            && self.daily.used_bytes >= limit
        {
            return Some(BudgetExceeded {
                period: BudgetKind::Daily,
                used_bytes: self.daily.used_bytes,
                limit_bytes: limit,
            });
        }
        if let Some(limit) = limits.monthly_bytes
            && self.monthly.used_bytes >= limit
        {
            return Some(BudgetExceeded {
                period: BudgetKind::Monthly,
                used_bytes: self.monthly.used_bytes,
                limit_bytes: limit,
            });
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetLimits {
    pub daily_bytes: Option<u64>,
    pub monthly_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    Daily,
    Monthly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetExceeded {
    pub period: BudgetKind,
    pub used_bytes: u64,
    pub limit_bytes: u64,
}

/// Per-profile counters, so switching profiles does not mix their budgets.
pub type BudgetStates = HashMap<String, BudgetState>;

#[cfg(test)]
mod tests {
    use super::{BudgetKind, BudgetLimits, BudgetState};

    #[test]
    fn a_period_change_resets_only_the_counter_that_rolled_over() {
        let mut state = BudgetState::default();
        state.record("2026-09-30", "2026-09", 100);
        state.record("2026-10-01", "2026-10", 50);
        assert_eq!(state.daily.used_bytes, 50);
        assert_eq!(state.monthly.used_bytes, 50);

        let mut state = BudgetState::default();
        state.record("2026-09-29", "2026-09", 100);
        state.record("2026-09-30", "2026-09", 50);
        assert_eq!(state.daily.used_bytes, 50, "the day rolled over");
        assert_eq!(state.monthly.used_bytes, 150, "the month did not");
    }

    #[test]
    fn a_shrinking_total_adds_nothing() {
        let mut state = BudgetState::default();
        assert_eq!(state.delta_from_total(1_000), 1_000);
        assert_eq!(state.delta_from_total(1_500), 500);
        // A deleted package lowers the total; that is not negative traffic.
        assert_eq!(state.delta_from_total(200), 0);
        assert_eq!(state.delta_from_total(300), 100);
    }

    #[test]
    fn the_daily_budget_is_reported_before_the_monthly_one() {
        let mut state = BudgetState::default();
        state.record("2026-09-03", "2026-09", 1_000);
        let limits = BudgetLimits {
            daily_bytes: Some(500),
            monthly_bytes: Some(500),
        };
        let exceeded = state.exhausted(&limits).expect("exhausted");
        assert_eq!(exceeded.period, BudgetKind::Daily);
        assert_eq!(exceeded.limit_bytes, 500);
        assert!(state.exhausted(&BudgetLimits::default()).is_none());
    }
}
