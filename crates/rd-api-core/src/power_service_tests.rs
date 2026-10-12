//! A countdown is one `power.changed` when it starts and one when it ends (RD-1260-04).

use rd_power::{CompletionAction, PendingAction};

use super::CountdownNotice;

fn countdown(cycle: u64) -> PendingAction {
    PendingAction {
        action: CompletionAction::Shutdown,
        runs_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).expect("time"),
        cycle,
    }
}

/// Sixty seconds of countdown at the supervisor's five-second tick: twelve ticks, which used to
/// be twelve events and twelve "power action pending" notifications.
#[test]
fn a_countdown_is_announced_once_however_many_ticks_it_lasts() {
    let mut notice = CountdownNotice::default();
    let pending = countdown(3);
    let announced: Vec<_> = (0..12)
        .filter_map(|_| notice.observe(Some(&pending), false, false))
        .collect();
    assert_eq!(announced.len(), 1, "{announced:?}");
    assert_eq!(announced[0]["state"], "pending");
    assert_eq!(announced[0]["pending"], "shutdown");
}

#[test]
fn the_end_of_a_countdown_says_how_it_ended() {
    for (ran, busy, state, reason) in [
        (true, false, "running", None),
        (false, false, "cancelled", Some("cancelled")),
        (false, true, "cancelled", Some("new_work")),
    ] {
        let mut notice = CountdownNotice::default();
        let pending = countdown(1);
        assert!(notice.observe(Some(&pending), false, false).is_some());
        let ended = notice
            .observe(None, ran, busy)
            .expect("the end is announced");
        assert_eq!(ended["state"], state);
        assert_eq!(ended["action"], "shutdown");
        assert_eq!(ended["reason"].as_str(), reason);
        // Ticks after the end are silent again.
        assert_eq!(notice.observe(None, false, false), None);
    }
}

#[test]
fn a_new_countdown_is_announced_and_an_idle_queue_never() {
    let mut notice = CountdownNotice::default();
    assert_eq!(notice.observe(None, false, false), None);
    assert!(notice.observe(Some(&countdown(1)), false, false).is_some());
    let next = notice
        .observe(Some(&countdown(2)), false, false)
        .expect("a countdown of the next cycle is news");
    assert_eq!(next["state"], "pending");
}
