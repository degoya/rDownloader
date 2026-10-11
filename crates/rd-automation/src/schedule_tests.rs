use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, TimeZone, Utc};

use super::{GRACE_SECONDS, Schedule};

fn zone() -> FixedOffset {
    FixedOffset::east_opt(2 * 3_600).expect("zone")
}

fn at(text: &str) -> DateTime<Utc> {
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").expect("naive time");
    zone()
        .from_local_datetime(&naive)
        .single()
        .expect("local time")
        .with_timezone(&Utc)
}

fn cron(expression: &str) -> Schedule {
    Schedule::Cron {
        expression: expression.to_owned(),
    }
}

#[test]
fn a_cron_slot_is_due_at_its_time_in_the_service_s_zone() {
    let daily = cron("0 6 * * *");
    // Six in the morning where the service runs, not six UTC.
    let slot = daily
        .due_slot(at("2026-10-10 06:00:05"), &zone())
        .expect("due");
    assert_eq!(slot.at, at("2026-10-10 06:00:00"));
    assert_eq!(slot.label(), "2026-10-10T06:00");
    assert!(daily.due_slot(at("2026-10-10 05:59:55"), &zone()).is_none());
    assert!(daily.due_slot(at("2026-10-10 06:00:05"), &Utc).is_none());
}

#[test]
fn a_slot_missed_by_more_than_the_grace_is_not_caught_up() {
    let daily = cron("0 6 * * *");
    let late = at("2026-10-10 06:00:00") + Duration::seconds(GRACE_SECONDS + 1);
    assert!(daily.due_slot(late, &zone()).is_none());
    let hourly = Schedule::Interval { minutes: 60 };
    assert!(
        hourly
            .due_slot(at("2026-10-10 07:30:00"), &zone())
            .is_none()
    );
}

#[test]
fn the_newest_slot_inside_the_window_is_the_one_due() {
    let every_minute = cron("* * * * *");
    let slot = every_minute
        .due_slot(at("2026-10-10 06:01:30"), &zone())
        .expect("due");
    assert_eq!(slot.at, at("2026-10-10 06:01:00"));
}

#[test]
fn an_interval_counts_from_midnight() {
    let six_hours = Schedule::Interval { minutes: 360 };
    let slot = six_hours
        .due_slot(at("2026-10-10 12:01:00"), &zone())
        .expect("due");
    assert_eq!(slot.label(), "2026-10-10T12:00");
    assert!(
        six_hours
            .due_slot(at("2026-10-10 13:00:00"), &zone())
            .is_none()
    );
    assert_eq!(
        six_hours.next_after(at("2026-10-10 12:01:00"), &zone()),
        Some(at("2026-10-10 18:00:00"))
    );
}

#[test]
fn the_same_slot_seen_twice_has_the_same_name() {
    // What keeps a restart inside the window from running a slot a second time: the key the
    // engine builds from the label is the same on both sides of the restart.
    let hourly = Schedule::Interval { minutes: 60 };
    let before = hourly
        .due_slot(at("2026-10-10 09:00:10"), &zone())
        .expect("due");
    let after_restart = hourly
        .due_slot(at("2026-10-10 09:01:40"), &zone())
        .expect("due");
    assert_eq!(before, after_restart);
}

#[test]
fn what_names_no_time_is_refused() {
    let now = at("2026-10-10 08:00:00");
    for bad in [
        cron(""),
        cron("every day at six"),
        cron("0 0 6 * * *"),
        cron("0 25 * * *"),
        cron("0 0 30 2 *"),
        Schedule::Interval { minutes: 0 },
        Schedule::Interval { minutes: 1_441 },
    ] {
        assert!(bad.validate(now, &zone()).is_err(), "{bad:?} was accepted");
    }
    for good in [
        cron("30 7 * * 1-5"),
        cron("@daily"),
        Schedule::Interval { minutes: 15 },
    ] {
        assert!(good.validate(now, &zone()).is_ok(), "{good:?} was refused");
    }
}

#[test]
fn the_next_cron_slot_is_reported_for_the_dry_run() {
    assert_eq!(
        cron("30 7 * * 1-5").next_after(at("2026-10-10 08:00:00"), &zone()),
        // 2026-10-10 is a Saturday.
        Some(at("2026-10-12 07:30:00"))
    );
}
