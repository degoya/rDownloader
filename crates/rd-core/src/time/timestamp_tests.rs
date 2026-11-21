//! `Timestamp` against golden literals and against chrono 0.4 itself.

use chrono::{DateTime, Datelike, SubsecRound, TimeZone, Utc};

use super::{SecondsFormat, SignedDuration, Timestamp};

fn ours(value: Timestamp) -> (i64, u32) {
    (value.unix_seconds(), value.subsec_nanos())
}

fn theirs(value: &DateTime<Utc>) -> (i64, u32) {
    (value.timestamp(), value.timestamp_subsec_nanos())
}

fn at(seconds: i64, nanos: u32) -> Timestamp {
    Timestamp::from_unix(seconds, nanos).expect("an instant in range")
}

fn chrono_at(seconds: i64, nanos: u32) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, nanos).expect("an instant in range")
}

/// Every fraction width, both sides of the epoch, the ends of the years 0..=9999 and beyond.
fn instants() -> Vec<(i64, u32)> {
    let mut list = vec![
        (0, 0),
        (1_700_000_000, 0),
        (1_700_000_000, 100_000_000),
        (1_700_000_000, 123_000_000),
        (1_700_000_000, 123_400_000),
        (1_700_000_000, 123_456_000),
        (1_700_000_000, 123_456_700),
        (1_700_000_000, 123_456_789),
        (1_700_000_000, 1),
        (1_700_000_000, 1_000),
        (1_700_000_000, 999_999_999),
        (-1, 0),
        (-1, 500_000_000),
        (-3_155_760_000, 250),
        (-62_167_219_200, 0),
        (-62_135_596_800, 7_000_000),
        (-62_330_000_000, 0),
        (253_402_000_000, 999_999_999),
    ];
    for end in [Timestamp::MIN, Timestamp::MAX] {
        list.push(ours(end));
    }
    list
}

#[test]
fn golden_literals() {
    let cases = [
        (
            (1_700_000_000, 0),
            "2023-11-14T22:13:20Z",
            "2023-11-14T22:13:20+00:00",
        ),
        (
            (1_700_000_000, 100_000_000),
            "2023-11-14T22:13:20.100Z",
            "2023-11-14T22:13:20.100+00:00",
        ),
        (
            (1_700_000_000, 123_456_000),
            "2023-11-14T22:13:20.123456Z",
            "2023-11-14T22:13:20.123456+00:00",
        ),
        (
            (1_700_000_000, 1),
            "2023-11-14T22:13:20.000000001Z",
            "2023-11-14T22:13:20.000000001+00:00",
        ),
        (
            (-1, 500_000_000),
            "1969-12-31T23:59:59.500Z",
            "1969-12-31T23:59:59.500+00:00",
        ),
        (
            (-62_167_219_200, 0),
            "0000-01-01T00:00:00Z",
            "0000-01-01T00:00:00+00:00",
        ),
        ((0, 0), "1970-01-01T00:00:00Z", "1970-01-01T00:00:00+00:00"),
    ];
    for ((seconds, nanos), json, sqlite) in cases {
        let value = at(seconds, nanos);
        assert_eq!(
            serde_json::to_string(&value).expect("json"),
            format!("\"{json}\"")
        );
        assert_eq!(value.to_string(), json);
        assert_eq!(format!("{value:?}"), json);
        assert_eq!(value.to_rfc3339(), sqlite);
    }
    assert_eq!(Timestamp::default(), Timestamp::UNIX_EPOCH);
    assert_eq!(Timestamp::MIN.to_string(), "-9999-01-02T01:59:59Z");
    assert_eq!(Timestamp::MAX.to_string(), "9999-12-30T22:00:00.999999999Z");
}

#[test]
fn every_representation_equals_chrono() {
    let forms = [
        SecondsFormat::Secs,
        SecondsFormat::Millis,
        SecondsFormat::Micros,
        SecondsFormat::Nanos,
        SecondsFormat::AutoSi,
    ];
    let chrono_forms = [
        chrono::SecondsFormat::Secs,
        chrono::SecondsFormat::Millis,
        chrono::SecondsFormat::Micros,
        chrono::SecondsFormat::Nanos,
        chrono::SecondsFormat::AutoSi,
    ];
    for (seconds, nanos) in instants() {
        let value = at(seconds, nanos);
        let reference = chrono_at(seconds, nanos);
        let case = format!("{seconds}.{nanos}");
        assert_eq!(
            serde_json::to_string(&value).expect("json"),
            serde_json::to_string(&reference).expect("json"),
            "{case}"
        );
        assert_eq!(value.to_rfc3339(), reference.to_rfc3339(), "{case}");
        for (form, chrono_form) in forms.into_iter().zip(chrono_forms) {
            for use_z in [false, true] {
                assert_eq!(
                    value.to_rfc3339_opts(form, use_z),
                    reference.to_rfc3339_opts(chrono_form, use_z),
                    "{case} {form:?} {use_z}"
                );
            }
        }
        assert_eq!(value.unix_seconds(), reference.timestamp(), "{case}");
        assert_eq!(value.unix_millis(), reference.timestamp_millis(), "{case}");
        assert_eq!(
            value.subsec_nanos(),
            reference.timestamp_subsec_nanos(),
            "{case}"
        );
        assert_eq!(
            value.subsec_millis(),
            reference.timestamp_subsec_millis(),
            "{case}"
        );
        assert_eq!(i32::from(value.date().year()), reference.year(), "{case}");
        for digits in 0..=9u16 {
            assert_eq!(
                ours(value.trunc_subsecs(digits)),
                theirs(&reference.trunc_subsecs(digits)),
                "{case} trunc {digits}"
            );
        }
        // serde both ways, and what chrono reads back from our text.
        let text = serde_json::to_string(&value).expect("json");
        assert_eq!(
            serde_json::from_str::<Timestamp>(&text).expect("back"),
            value,
            "{case}"
        );
        let read: DateTime<Utc> = serde_json::from_str(&text).expect("chrono reads it");
        assert_eq!(read, reference, "{case}");
    }
}

#[test]
fn unix_conversions_match_chrono() {
    assert_eq!(Timestamp::from_unix(0, 1_000_000_000), None);
    assert_eq!(Timestamp::from_unix_seconds(i64::MAX), None);
    for millis in [0i64, 1, -1, 1_700_000_000_123, -1_700_000_000_123] {
        let value = Timestamp::from_unix_millis(millis).expect("in range");
        let reference = DateTime::<Utc>::from_timestamp_millis(millis).expect("in range");
        assert_eq!(ours(value), theirs(&reference), "{millis}");
        assert_eq!(value.unix_millis(), millis);
    }
    assert_eq!(at(-1, 500_000_000).unix_nanos(), -500_000_000);
}

#[test]
fn civil_constructors_match_chrono() {
    let cases = [
        (2026, 11, 21, 14, 0, 0),
        (2024, 2, 29, 23, 59, 59),
        (2023, 2, 29, 0, 0, 0),
        (2026, 4, 31, 0, 0, 0),
        (2026, 1, 1, 24, 0, 0),
        (2026, 1, 1, 0, 60, 0),
        (2026, 13, 1, 0, 0, 0),
        (0, 1, 1, 0, 0, 0),
    ];
    for (year, month, day, hour, minute, second) in cases {
        let value = Timestamp::from_ymd_hms(year, month, day, hour, minute, second);
        let reference = Utc
            .with_ymd_and_hms(year, month, day, hour, minute, second)
            .single();
        assert_eq!(
            value.map(ours),
            reference.as_ref().map(theirs),
            "{year}-{month}-{day}"
        );
    }
    let value = at(1_700_000_000, 5);
    assert_eq!(Timestamp::from_utc_datetime(value.utc_datetime()), value);
    assert_eq!(value.date().to_string(), "2023-11-14");
}

#[test]
fn arithmetic_matches_chrono() {
    let value = at(1_700_000_000, 250_000_000);
    let reference = chrono_at(1_700_000_000, 250_000_000);
    let steps = [
        (SignedDuration::from_secs(90), chrono::Duration::seconds(90)),
        (SignedDuration::from_mins(-7), chrono::Duration::minutes(-7)),
        (
            SignedDuration::from_hours(24 * 3),
            chrono::Duration::days(3),
        ),
        (
            SignedDuration::from_millis(1_500),
            chrono::Duration::milliseconds(1_500),
        ),
        (SignedDuration::from_hours(-5), chrono::Duration::hours(-5)),
    ];
    for (step, chrono_step) in steps {
        assert_eq!(
            ours(value + step),
            theirs(&(reference + chrono_step)),
            "{step:?}"
        );
        assert_eq!(
            ours(value - step),
            theirs(&(reference - chrono_step)),
            "{step:?}"
        );
        let mut moved = value;
        moved += step;
        moved -= step;
        assert_eq!(moved, value);
        assert_eq!(value.checked_add(step), Some(value + step));
        let later = value + step;
        let chrono_difference = (reference + chrono_step) - reference;
        assert_eq!(
            (later - value).as_nanos(),
            i128::from(chrono_difference.num_nanoseconds().expect("fits"))
        );
        assert_eq!(later.duration_since(value), step);
    }
    assert_eq!(
        Timestamp::MAX.checked_add(SignedDuration::from_secs(1)),
        None
    );
    assert_eq!(
        Timestamp::MIN.checked_sub(SignedDuration::from_secs(1)),
        None
    );
    assert_eq!(
        Timestamp::MAX.saturating_add(SignedDuration::from_secs(1)),
        Timestamp::MAX
    );
    assert_eq!(
        Timestamp::MIN.saturating_sub(SignedDuration::from_secs(1)),
        Timestamp::MIN
    );
    assert_eq!(
        Timestamp::MIN.saturating_add(SignedDuration::from_secs(-1)),
        Timestamp::MIN
    );
}

/// The reason for `+00:00` in SQLite: that text sorts as the instants do, within one second
/// too. (With `Z`, `…:00Z` would sort after `…:00.5Z`.)
#[test]
fn stored_text_orders_as_the_instants() {
    let mut values: Vec<Timestamp> = [
        (1_700_000_000, 0),
        (1_700_000_000, 5),
        (1_700_000_000, 1_000),
        (1_700_000_000, 1_000_000),
        (1_700_000_000, 100_000_000),
        (1_700_000_000, 120_000_000),
        (1_700_000_000, 123_456_789),
        (1_700_000_000, 500_000_000),
        (1_700_000_000, 999_999_999),
        (1_700_000_001, 0),
        (1_699_999_999, 999_000_000),
    ]
    .into_iter()
    .map(|(seconds, nanos)| at(seconds, nanos))
    .collect();
    let mut texts: Vec<String> = values.iter().map(|value| value.to_rfc3339()).collect();
    values.sort();
    texts.sort();
    let sorted: Vec<String> = values.iter().map(|value| value.to_rfc3339()).collect();
    assert_eq!(texts, sorted);
}

#[test]
fn strftime_matches_chrono_for_the_directives_in_use() {
    let value = at(1_700_000_000, 123_456_789);
    let reference = chrono_at(1_700_000_000, 123_456_789);
    for pattern in [
        "%Y%m%dT%H%M%SZ",
        "%Y-%m-%d",
        "%H:%M",
        "%Y%m%d-%H%M%S",
        "%Y-%m",
        "%Y-%m-%dT%H:%M:%SZ",
        "%Y%m%dT%H%M%S%.9fZ",
        "%Y%m%dT%H%M%S%3fZ",
        "%Y-%m-%d %H:%M UTC",
        "%b %-d %H:%M",
    ] {
        assert_eq!(
            value.strftime(pattern).to_string(),
            reference.format(pattern).to_string(),
            "{pattern}"
        );
    }
}
