use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::{AutoRestart, RestartMoment};
use crate::auto_install::{InstallWindow, QUIET_PERIOD, Wait};

fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 2, 0, 0)
        .single()
        .expect("time")
        + TimeDelta::minutes(minutes)
}

/// Switched on, pending, possible, idle, no window: everything a restart asks for.
fn ready() -> RestartMoment {
    RestartMoment {
        enabled: true,
        pending: true,
        can_restart: true,
        busy: false,
        local_minute: 4 * 60,
        window: None,
    }
}

/// After a quiet period the service restarts; not before.
#[test]
fn nothing_running_for_the_quiet_period_restarts() {
    let mut auto = AutoRestart::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(4)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(0) + QUIET_PERIOD), Ok(()));
}

/// A running transfer waits, and starts the quiet period over once it ends.
#[test]
fn a_running_transfer_holds_the_restart_back() {
    let mut auto = AutoRestart::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    let busy = RestartMoment {
        busy: true,
        ..ready()
    };
    assert_eq!(auto.decide(&busy, at(6)), Err(Wait::Busy));
    assert_eq!(auto.decide(&busy, at(30)), Err(Wait::Busy));
    assert_eq!(auto.decide(&ready(), at(31)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(36)), Ok(()));
}

/// Off never restarts, however long it has been quiet; switched on after a quiet night it
/// restarts at once.
#[test]
fn off_never_restarts() {
    let mut auto = AutoRestart::default();
    let off = RestartMoment {
        enabled: false,
        ..ready()
    };
    for minute in [0, 10, 600] {
        assert_eq!(auto.decide(&off, at(minute)), Err(Wait::Off));
    }
    assert_eq!(auto.decide(&ready(), at(601)), Ok(()));
}

/// Nothing pending, or an update being installed, restarts nothing.
#[test]
fn nothing_pending_or_an_install_under_way_restarts_nothing() {
    let mut auto = AutoRestart::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    let calm = RestartMoment {
        pending: false,
        ..ready()
    };
    assert_eq!(auto.decide(&calm, at(60)), Err(Wait::NothingPending));
    let installing = RestartMoment {
        can_restart: false,
        ..ready()
    };
    assert_eq!(auto.decide(&installing, at(60)), Err(Wait::Installing));
    assert_eq!(auto.decide(&ready(), at(60)), Ok(()));
}

/// The automatic install's window holds the restart until it opens.
#[test]
fn the_time_window_holds_the_restart_until_it_opens() {
    let window = InstallWindow {
        start_minute: 3 * 60,
        end_minute: 6 * 60,
    };
    let mut auto = AutoRestart::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    let before = RestartMoment {
        window: Some(window),
        local_minute: 2 * 60 + 59,
        ..ready()
    };
    assert_eq!(auto.decide(&before, at(30)), Err(Wait::OutsideWindow));
    let inside = RestartMoment {
        local_minute: 3 * 60,
        ..before
    };
    assert_eq!(auto.decide(&inside, at(31)), Ok(()));
}
