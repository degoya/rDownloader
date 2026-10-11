//! The download window and the schedule's pause, evaluated at injected instants (RD-1240-30).

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use rd_core::{BandwidthWindowId, DownloadWindow, WeeklyWindow};

use super::{DownloadHold, package_hold, window_open};
use crate::{BandwidthProfile, DaySet, ScheduleWindow, WeeklySchedule};

const BERLIN: Tz = Tz::Europe__Berlin;

fn window(start: u16, end: u16, ignore_schedule_pause: bool) -> DownloadWindow {
    DownloadWindow {
        windows: vec![WeeklyWindow {
            days: DaySet::EVERY_DAY.0,
            start_minute: start,
            end_minute: end,
        }],
        ignore_schedule_pause,
    }
}

#[test]
fn a_window_that_wraps_midnight_is_open_on_both_sides_of_it() {
    let night = window(22 * 60, 6 * 60, false);
    // 2026-01-15 23:30 Berlin = 22:30 UTC (CET).
    assert!(window_open(
        &night,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 15, 22, 30, 0).unwrap()
    ));
    // 05:59 Berlin the next morning = 04:59 UTC.
    assert!(window_open(
        &night,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 16, 4, 59, 0).unwrap()
    ));
    // 06:00 Berlin: the exclusive end.
    assert!(!window_open(
        &night,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 16, 5, 0, 0).unwrap()
    ));
    // 12:00 Berlin.
    assert!(!window_open(
        &night,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 16, 11, 0, 0).unwrap()
    ));
}

#[test]
fn a_wrapping_window_belongs_to_the_day_it_starts_on() {
    // Friday 22:00–02:00 only (bit 4 = Friday).
    let friday = DownloadWindow {
        windows: vec![WeeklyWindow {
            days: 1 << 4,
            start_minute: 22 * 60,
            end_minute: 2 * 60,
        }],
        ignore_schedule_pause: false,
    };
    // Saturday 2026-01-17 01:00 Berlin = 00:00 UTC: still Friday's window.
    assert!(window_open(
        &friday,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 17, 0, 0, 0).unwrap()
    ));
    // Thursday 2026-01-15 23:00 Berlin: not a Friday.
    assert!(!window_open(
        &friday,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 15, 22, 0, 0).unwrap()
    ));
}

#[test]
fn the_spring_forward_hour_closes_the_window_on_the_local_clock() {
    // Open 00:00–03:00; Germany skips 02:00–03:00 local on 2026-03-29.
    let early = window(0, 3 * 60, false);
    // 00:30 UTC = 01:30 CET: open.
    assert!(window_open(
        &early,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 3, 29, 0, 30, 0).unwrap()
    ));
    // 01:00 UTC = 03:00 CEST: the clock jumped past the end, so it is closed.
    assert!(!window_open(
        &early,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 3, 29, 1, 0, 0).unwrap()
    ));
}

#[test]
fn the_autumn_hour_that_happens_twice_opens_the_window_both_times() {
    // Open 02:30–06:00; Germany repeats 02:00–03:00 local on 2026-10-25.
    let late = window(2 * 60 + 30, 6 * 60, false);
    // 00:45 UTC = 02:45 CEST, first pass.
    assert!(window_open(
        &late,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 10, 25, 0, 45, 0).unwrap()
    ));
    // 01:15 UTC = 02:15 CET, the repeated hour before the start.
    assert!(!window_open(
        &late,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 10, 25, 1, 15, 0).unwrap()
    ));
    // 01:45 UTC = 02:45 CET, second pass.
    assert!(window_open(
        &late,
        BERLIN,
        Utc.with_ymd_and_hms(2026, 10, 25, 1, 45, 0).unwrap()
    ));
}

#[test]
fn a_window_without_spans_is_always_open() {
    assert!(window_open(
        &DownloadWindow::default(),
        BERLIN,
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
    ));
}

#[test]
fn the_schedule_pause_holds_a_package_unless_it_ignores_it() {
    let noon = Utc.with_ymd_and_hms(2026, 1, 15, 11, 0, 0).unwrap();
    assert_eq!(
        package_hold(None, true, BERLIN, noon),
        Some(DownloadHold::Schedule)
    );
    assert_eq!(package_hold(None, false, BERLIN, noon), None);
    let urgent = DownloadWindow {
        windows: Vec::new(),
        ignore_schedule_pause: true,
    };
    assert_eq!(package_hold(Some(&urgent), true, BERLIN, noon), None);
    // A package that ignores the pause still keeps to its own window.
    let night_only = window(22 * 60, 6 * 60, true);
    assert_eq!(
        package_hold(Some(&night_only), true, BERLIN, noon),
        Some(DownloadHold::Window)
    );
    let late = Utc.with_ymd_and_hms(2026, 1, 15, 22, 0, 0).unwrap();
    assert_eq!(package_hold(Some(&night_only), true, BERLIN, late), None);
    // Without the switch the schedule's pause holds it inside its window as well.
    let obeying = window(22 * 60, 6 * 60, false);
    assert_eq!(
        package_hold(Some(&obeying), true, BERLIN, late),
        Some(DownloadHold::Schedule)
    );
}

/// The owner's case: downloads start at 22:00 and pause at 06:00. A day profile that pauses
/// downloads covers 06:00–22:00; outside it the default profile lets them run.
#[test]
fn a_day_profile_that_pauses_downloads_leaves_the_night_to_them() {
    let mut day = BandwidthProfile::new("Day".to_owned());
    day.pause_downloads = true;
    let schedule = WeeklySchedule {
        timezone: BERLIN,
        default_profile_id: None,
        windows: vec![ScheduleWindow {
            id: BandwidthWindowId::new(),
            profile_id: day.id,
            days: DaySet::EVERY_DAY,
            start_minute: 6 * 60,
            end_minute: 22 * 60,
            priority: 0,
            enabled: true,
        }],
    };
    let pauses = |now| schedule.active_at(now) == Some(day.id) && day.pause_downloads;
    // 12:00 Berlin: paused.
    assert!(pauses(Utc.with_ymd_and_hms(2026, 1, 15, 11, 0, 0).unwrap()));
    // 22:00 Berlin: the window ends, downloads run.
    let evening = Utc.with_ymd_and_hms(2026, 1, 15, 21, 0, 0).unwrap();
    assert!(!pauses(evening));
    assert_eq!(
        schedule.next_switch_after(Utc.with_ymd_and_hms(2026, 1, 15, 20, 59, 30).unwrap()),
        Some(evening)
    );
    // 03:00 Berlin: still the night.
    assert!(!pauses(Utc.with_ymd_and_hms(2026, 1, 16, 2, 0, 0).unwrap()));
}
