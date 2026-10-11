use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::{AutoInstall, InstallWindow, Moment, QUIET_PERIOD, Wait};

fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 2, 0, 0)
        .single()
        .expect("time")
        + TimeDelta::minutes(minutes)
}

/// Switched on, installable, offered, idle, no window: everything an install asks for.
fn ready() -> Moment<'static> {
    Moment {
        enabled: true,
        installs_itself: true,
        offered: Some("1.25.0"),
        installing: false,
        failed_before: false,
        busy: false,
        local_minute: 4 * 60,
        window: None,
    }
}

/// After a quiet period the offered version is installed; not before.
#[test]
fn nothing_running_for_the_quiet_period_installs_the_offer() {
    let mut auto = AutoInstall::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(4)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(0) + QUIET_PERIOD), Ok("1.25.0"));
}

/// Something running waits, and starts the quiet period over once it ends.
#[test]
fn a_running_transfer_waits_and_restarts_the_quiet_period() {
    let mut auto = AutoInstall::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    let busy = Moment {
        busy: true,
        ..ready()
    };
    assert_eq!(auto.decide(&busy, at(6)), Err(Wait::Busy));
    assert_eq!(auto.decide(&ready(), at(7)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(11)), Err(Wait::Settling));
    assert_eq!(auto.decide(&ready(), at(12)), Ok("1.25.0"));
}

/// Off never installs, however long it has been quiet; switched on later after a quiet night it
/// installs at once.
#[test]
fn off_never_installs() {
    let mut auto = AutoInstall::default();
    let off = Moment {
        enabled: false,
        ..ready()
    };
    for minute in [0, 10, 600] {
        assert_eq!(auto.decide(&off, at(minute)), Err(Wait::Off));
    }
    assert_eq!(auto.decide(&ready(), at(601)), Ok("1.25.0"));
}

#[test]
fn an_installation_that_does_not_install_itself_never_installs() {
    let mut auto = AutoInstall::default();
    let docker = Moment {
        installs_itself: false,
        ..ready()
    };
    assert_eq!(auto.decide(&docker, at(0)), Err(Wait::Unsupported));
    assert_eq!(auto.decide(&docker, at(60)), Err(Wait::Unsupported));
}

#[test]
fn nothing_offered_running_or_failed_before_installs_nothing() {
    let mut auto = AutoInstall::default();
    let cases = [
        (
            Moment {
                offered: None,
                ..ready()
            },
            Wait::NothingOffered,
        ),
        (
            Moment {
                installing: true,
                ..ready()
            },
            Wait::Installing,
        ),
        (
            Moment {
                failed_before: true,
                ..ready()
            },
            Wait::FailedBefore,
        ),
    ];
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    for (moment, wait) in cases {
        assert_eq!(auto.decide(&moment, at(60)), Err(wait), "{wait:?}");
    }
}

/// A window from 03:00 to 06:00 installs inside it only; one over midnight wraps.
#[test]
fn the_time_window_holds_the_install_until_it_opens() {
    let window = InstallWindow {
        start_minute: 3 * 60,
        end_minute: 6 * 60,
    };
    let mut auto = AutoInstall::default();
    assert_eq!(auto.decide(&ready(), at(0)), Err(Wait::Settling));
    let before = Moment {
        window: Some(window),
        local_minute: 2 * 60 + 59,
        ..ready()
    };
    assert_eq!(auto.decide(&before, at(30)), Err(Wait::OutsideWindow));
    let inside = Moment {
        local_minute: 3 * 60,
        ..before
    };
    assert_eq!(auto.decide(&inside, at(31)), Ok("1.25.0"));
    let after = Moment {
        local_minute: 6 * 60,
        ..before
    };
    assert_eq!(auto.decide(&after, at(32)), Err(Wait::OutsideWindow));

    let night = InstallWindow {
        start_minute: 23 * 60,
        end_minute: 60,
    };
    assert!(night.covers(23 * 60) && night.covers(0) && night.covers(59));
    assert!(!night.covers(60) && !night.covers(22 * 60 + 59));
}

#[test]
fn an_empty_or_out_of_day_window_is_invalid() {
    let window = |start_minute, end_minute| InstallWindow {
        start_minute,
        end_minute,
    };
    assert!(window(180, 360).is_valid());
    assert!(window(1380, 60).is_valid());
    assert!(!window(180, 180).is_valid());
    assert!(!window(1440, 60).is_valid());
    assert!(!window(0, 1440).is_valid());
}
