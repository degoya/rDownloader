//! The budget gate and what it announces (RD-120-62, RD-120-64).

use rd_core::BandwidthProfileId;
use rd_limits::{BudgetExceeded, BudgetKind};

use super::BandwidthState;

const DAY: &str = "2026-09-24";
const MONTH: &str = "2026-09";

fn exceeded(period: BudgetKind) -> Option<BudgetExceeded> {
    Some(BudgetExceeded {
        period,
        used_bytes: 600,
        limit_bytes: 500,
    })
}

#[test]
fn a_budget_running_out_is_announced_once_on_the_edge() {
    let mut state = BandwidthState::default();
    let profile = Some(BandwidthProfileId::new());
    let event = state
        .settle_exceeded(profile, exceeded(BudgetKind::Daily), DAY, MONTH)
        .expect("the budget ran out");
    assert_eq!(event.kind, rd_core::EventKind::BandwidthChanged);
    assert_eq!(event.payload["entity"], "budget");
    assert_eq!(event.payload["exhausted"], true);
    assert_eq!(event.payload["period"], "daily");
    assert_eq!(event.payload["limit_bytes"], 500);
    // Still used up on the next tick: nothing new to say.
    assert!(
        state
            .settle_exceeded(profile, exceeded(BudgetKind::Daily), DAY, MONTH)
            .is_none()
    );
    // The monthly budget running out after the daily one is a second exhaustion.
    let monthly = state
        .settle_exceeded(profile, exceeded(BudgetKind::Monthly), DAY, MONTH)
        .expect("the monthly budget ran out");
    assert_eq!(monthly.payload["period"], "monthly");
    // Nothing used up before or after: silence.
    let mut quiet = BandwidthState::default();
    assert!(quiet.settle_exceeded(profile, None, DAY, MONTH).is_none());
}

#[test]
fn a_budget_coming_back_is_announced_but_not_as_exhausted() {
    let mut state = BandwidthState::default();
    let profile = Some(BandwidthProfileId::new());
    state.settle_exceeded(profile, exceeded(BudgetKind::Daily), DAY, MONTH);
    let event = state
        .settle_exceeded(profile, None, "2026-09-25", MONTH)
        .expect("the budget came back");
    assert_eq!(event.payload["entity"], "budget");
    assert_eq!(event.payload["exhausted"], false);
    assert!(state.exceeded.is_none());
}

#[test]
fn the_gate_clears_with_its_profile_and_is_not_announced_again() {
    let mut state = BandwidthState::default();
    let capped = Some(BandwidthProfileId::new());
    assert!(
        state
            .settle_exceeded(capped, exceeded(BudgetKind::Daily), DAY, MONTH)
            .is_some()
    );
    // The profile ends: the gate opens, and only the interface hears about it.
    let cleared = state
        .settle_exceeded(None, None, DAY, MONTH)
        .expect("the gate opened");
    assert_eq!(cleared.payload["exhausted"], false);
    assert!(state.exceeded.is_none());
    // Back the same day: the gate closes again, but the owner was told already.
    assert!(
        state
            .settle_exceeded(capped, exceeded(BudgetKind::Daily), DAY, MONTH)
            .is_none()
    );
    assert!(state.exceeded.is_some());
    // The next day is a new period, and its exhaustion is news.
    assert!(
        state
            .settle_exceeded(capped, exceeded(BudgetKind::Daily), "2026-09-25", MONTH)
            .is_some()
    );
}

#[test]
fn another_profile_is_measured_on_its_own_budget() {
    let mut state = BandwidthState::default();
    let capped = Some(BandwidthProfileId::new());
    let other = Some(BandwidthProfileId::new());
    state.settle_exceeded(capped, exceeded(BudgetKind::Daily), DAY, MONTH);
    // A switch to a profile with room left opens the gate.
    state.settle_exceeded(other, None, DAY, MONTH);
    assert!(state.exceeded.is_none());
    // A second profile running out the same day is its own exhaustion.
    assert!(
        state
            .settle_exceeded(other, exceeded(BudgetKind::Daily), DAY, MONTH)
            .is_some()
    );
}
