//! RD-1290-02: the traffic budget's placeholders, with a budget, without one and used up.

use rd_limits::{BudgetKind, BudgetReading};
use rd_notify::{NotificationEvent, Placeholder as P, Values};

use super::set_budget;

fn values() -> Values {
    Values::stamped(NotificationEvent::PackageCompleted, &chrono::Utc::now())
}

#[test]
fn a_budget_fills_its_four_placeholders_as_sizes() {
    let mut filled = values();
    set_budget(
        &mut filled,
        Some(BudgetReading {
            period: BudgetKind::Monthly,
            used_bytes: 1_610_612_736,
            limit_bytes: 10 * 1_073_741_824,
        }),
    );
    assert_eq!(filled.get(P::BudgetUsed), "1.5 GiB");
    assert_eq!(filled.get(P::BudgetLeft), "8.5 GiB");
    assert_eq!(filled.get(P::BudgetLimit), "10.0 GiB");
    assert_eq!(filled.get(P::BudgetPeriod), "monthly");
}

#[test]
fn without_a_budget_the_placeholders_stay_empty() {
    let mut filled = values();
    set_budget(&mut filled, None);
    for placeholder in [
        P::BudgetUsed,
        P::BudgetLeft,
        P::BudgetLimit,
        P::BudgetPeriod,
    ] {
        assert_eq!(filled.get(placeholder), "", "{placeholder:?}");
    }
    assert_eq!(
        filled.get(P::Event),
        "package_completed",
        "the rest is filled"
    );
}

#[test]
fn a_used_up_budget_has_nothing_left() {
    let mut filled = values();
    set_budget(
        &mut filled,
        Some(BudgetReading {
            period: BudgetKind::Daily,
            used_bytes: 3_000,
            limit_bytes: 2_048,
        }),
    );
    assert_eq!(filled.get(P::BudgetUsed), "2.9 KiB");
    assert_eq!(filled.get(P::BudgetLeft), "0 B");
    assert_eq!(filled.get(P::BudgetLimit), "2.0 KiB");
    assert_eq!(filled.get(P::BudgetPeriod), "daily");
}
