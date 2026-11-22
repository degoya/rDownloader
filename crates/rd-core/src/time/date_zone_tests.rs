//! `Date` against `chrono::NaiveDate`, `IanaZone` against `chrono_tz::Tz`.

use chrono::{NaiveDate, Offset, TimeZone, Utc};

use super::{Date, IanaZone, Timestamp};

fn naive(value: NaiveDate) -> (i32, u32, u32) {
    use chrono::Datelike;
    (value.year(), value.month(), value.day())
}

fn civil(value: Date) -> (i32, u32, u32) {
    let inner = value.as_jiff();
    (
        i32::from(inner.year()),
        u32::try_from(inner.month()).unwrap_or(0),
        u32::try_from(inner.day()).unwrap_or(0),
    )
}

#[test]
fn dates_write_as_chrono_writes() {
    for (year, month, day) in [
        (2026, 3, 9),
        (1970, 1, 1),
        (0, 1, 1),
        (-1, 12, 31),
        (9999, 12, 31),
        (2024, 2, 29),
    ] {
        let value = Date::from_ymd(year, month, day).expect("a date");
        let reference = NaiveDate::from_ymd_opt(year, month, day).expect("a date");
        assert_eq!(value.to_string(), reference.to_string());
        assert_eq!(format!("{value:?}"), format!("{reference:?}"));
        assert_eq!(
            serde_json::to_string(&value).expect("json"),
            serde_json::to_string(&reference).expect("json")
        );
    }
    assert_eq!(Date::from_ymd(2023, 2, 29), None);
    assert_eq!(
        Date::default().to_string(),
        NaiveDate::default().to_string()
    );
    assert_eq!(
        Date::from_ymd(2026, 11, 21).expect("a date").to_string(),
        "2026-11-21"
    );
}

#[test]
fn dates_read_as_chrono_reads() {
    let inputs = [
        "2026-03-09",
        "2026-3-9",
        " 2026-03-09 ",
        "2026 - 03 - 09",
        "+2026-03-09",
        "-0001-12-31",
        "2026-03-09T00:00",
        "2026-02-30",
        "2026-13-01",
        "26-03",
        "",
        "x",
        "12345-01-01",
    ];
    for input in inputs {
        let reference = input.parse::<NaiveDate>().map(naive).ok();
        let value = input.parse::<Date>().map(civil).ok();
        assert_eq!(value, reference, "{input:?}");
        let json = serde_json::to_string(input).expect("a JSON string");
        let reference = serde_json::from_str::<NaiveDate>(&json).map(naive).ok();
        let value = serde_json::from_str::<Date>(&json).map(civil).ok();
        assert_eq!(value, reference, "serde {input:?}");
    }
    let reference = serde_json::from_str::<NaiveDate>("\"x\"").expect_err("refused");
    let value = serde_json::from_str::<Date>("\"x\"").expect_err("refused");
    assert_eq!(value.to_string(), reference.to_string());
}

const ZONES: &[&str] = &[
    "UTC",
    "Europe/Berlin",
    "America/New_York",
    "Asia/Kolkata",
    "Australia/Lord_Howe",
    "Etc/GMT+5",
];

#[test]
fn zones_serialize_as_chrono_tz() {
    for name in ZONES {
        let value = IanaZone::parse(name).expect("a zone");
        let reference: chrono_tz::Tz = name.parse().expect("a zone");
        assert_eq!(value.name(), reference.name());
        assert_eq!(value.to_string(), reference.to_string());
        assert_eq!(
            serde_json::to_string(&value).expect("json"),
            serde_json::to_string(&reference).expect("json")
        );
        let json = serde_json::to_string(name).expect("a JSON string");
        assert_eq!(
            serde_json::from_str::<IanaZone>(&json).expect("back"),
            value
        );
    }
    assert_eq!(IanaZone::default(), IanaZone::utc());
    assert_eq!(IanaZone::utc().name(), "UTC");
}

#[test]
fn zones_refuse_what_chrono_tz_refuses() {
    for name in [
        "europe/berlin",
        "EUROPE/BERLIN",
        "Mars/Olympus",
        "",
        "Europe/Berlin ",
    ] {
        assert!(name.parse::<chrono_tz::Tz>().is_err(), "{name:?}");
        assert!(IanaZone::parse(name).is_err(), "{name:?}");
        let json = serde_json::to_string(name).expect("a JSON string");
        let reference = serde_json::from_str::<chrono_tz::Tz>(&json).expect_err("refused");
        let value = serde_json::from_str::<IanaZone>(&json).expect_err("refused");
        assert_eq!(value.to_string(), reference.to_string(), "{name:?}");
    }
    let reference = "Mars/Olympus"
        .parse::<chrono_tz::Tz>()
        .expect_err("refused");
    let value = IanaZone::parse("Mars/Olympus").expect_err("refused");
    assert_eq!(value.to_string(), reference.to_string());
}

/// The zone data agree: the same offset at the same instants, across the DST switches.
#[test]
fn zones_give_chrono_tz_offsets() {
    let instants = [
        (2026, 1, 15, 12, 0, 0),
        (2026, 3, 29, 0, 59, 59),
        (2026, 3, 29, 1, 0, 0),
        (2026, 7, 1, 12, 0, 0),
        (2026, 10, 25, 0, 59, 59),
        (2026, 10, 25, 1, 0, 0),
        (2026, 11, 1, 6, 0, 0),
    ];
    for name in ZONES {
        let zone = IanaZone::parse(name).expect("a zone");
        let reference: chrono_tz::Tz = name.parse().expect("a zone");
        for (year, month, day, hour, minute, second) in instants {
            let value = Timestamp::from_ymd_hms(year, month, day, hour, minute, second)
                .expect("an instant");
            let chrono_value = Utc
                .with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
                .expect("an instant");
            let offset = value
                .to_zoned_datetime(zone.as_jiff().clone())
                .offset()
                .seconds();
            let expected = chrono_value
                .with_timezone(&reference)
                .offset()
                .fix()
                .local_minus_utc();
            assert_eq!(
                offset, expected,
                "{name} {year}-{month}-{day}T{hour}:{minute}"
            );
        }
    }
}
