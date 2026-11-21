//! The parsers against chrono's: same instant, or an error on both sides.

use chrono::{DateTime, Utc};

use super::Timestamp;

fn ours(value: Timestamp) -> (i64, u32) {
    (value.unix_seconds(), value.subsec_nanos())
}

fn theirs(value: DateTime<Utc>) -> (i64, u32) {
    (value.timestamp(), value.timestamp_subsec_nanos())
}

const VALID_STRICT: &[&str] = &[
    "2023-11-14T22:13:20Z",
    "2023-11-14t22:13:20z",
    "2023-11-14 22:13:20+00:00",
    "2023-11-14T22:13:20.5+02:00",
    "2023-11-14T22:13:20.123456789123-01:30",
    "2023-11-14T22:13:20\u{2212}01:00",
    "2023-11-14T22:13:20+23:59",
    "0000-01-01T00:00:00Z",
    "2024-02-29T00:00:00.000Z",
];

const INVALID_STRICT: &[&str] = &[
    "",
    "2023-11-14T22:13:20",
    "2023-11-14T22:13:20+0100",
    "2023-11-14T22:13:20 UTC",
    " 2023-11-14T22:13:20Z",
    "2023-11-14T22:13:20Z ",
    "2023-1-14T22:13:20Z",
    "2023-11-14T22:13:20.Z",
    "2023-11-14T22:13:20+24:00",
    "2023-11-14T22:13:20+01:60",
    "2023-11-14T22:13:20+01",
    "2023-02-29T00:00:00Z",
    "2023-11-14X22:13:20Z",
    "2023-11-14T22:13Z",
];

/// What serde and `FromStr` read beyond strict RFC 3339 (chrono's relaxed grammar).
const VALID_RELAXED: &[&str] = &[
    "2023-11-14T22:13:20+0100",
    "2023-11-14T22:13:20 +01 00",
    "2023-11-14T22:13:20 UTC",
    "2023-11-14T22:13:20utc",
    "2023-11-14T22:13:20 Z",
    " 2023-11-14T22:13:20Z ",
    "2023-1-4T2:3:4Z",
    "2023 - 11 - 14T22 : 13 : 20Z",
    "2023-11-14T 22:13:20Z",
    "+2023-11-14T22:13:20Z",
    "-0001-12-31T23:59:59Z",
    "2023-11-14T22:13:20.1234567891234Z",
];

const INVALID_RELAXED: &[&str] = &[
    "",
    "x",
    "2023-11-14",
    "2023-11-14T22:13Z",
    "2023-11-14T22:13:20",
    "2023-11-14T22:13:20Zjunk",
    "2023-13-01T00:00:00Z",
    "2023-02-30T00:00:00Z",
    "2023-11-14T24:00:00Z",
    "2023-11-14T22:13:20+24:00",
    "2023-11-14T22:13:20+01:60",
    "2023-11-14X22:13:20Z",
    "2023-11-14T22:13:20.Z",
    "12345-01-01T00:00:00Z",
    "2023-11-14\t22:13:20Z",
];

#[test]
fn strict_parse_matches_parse_from_rfc3339() {
    for input in VALID_STRICT
        .iter()
        .chain(INVALID_STRICT)
        .chain(VALID_RELAXED)
    {
        let reference = DateTime::parse_from_rfc3339(input)
            .map(|parsed| theirs(parsed.with_timezone(&Utc)))
            .ok();
        let value = Timestamp::parse_rfc3339(input).map(ours).ok();
        assert_eq!(value, reference, "{input:?}");
    }
    for input in VALID_STRICT {
        assert!(Timestamp::parse_rfc3339(input).is_ok(), "{input:?}");
    }
    for input in INVALID_STRICT {
        assert!(Timestamp::parse_rfc3339(input).is_err(), "{input:?}");
    }
}

#[test]
fn relaxed_parse_matches_chrono_from_str_and_serde() {
    let all = VALID_STRICT
        .iter()
        .chain(INVALID_STRICT)
        .chain(VALID_RELAXED)
        .chain(INVALID_RELAXED);
    for input in all {
        let reference = input.parse::<DateTime<Utc>>().map(theirs).ok();
        let value = input.parse::<Timestamp>().map(ours).ok();
        assert_eq!(value, reference, "{input:?}");
        let json = serde_json::to_string(input).expect("a JSON string");
        let reference = serde_json::from_str::<DateTime<Utc>>(&json)
            .map(theirs)
            .ok();
        let value = serde_json::from_str::<Timestamp>(&json).map(ours).ok();
        assert_eq!(value, reference, "serde {input:?}");
    }
    for input in VALID_RELAXED {
        assert!(input.parse::<Timestamp>().is_ok(), "{input:?}");
    }
    for input in INVALID_RELAXED {
        assert!(input.parse::<Timestamp>().is_err(), "{input:?}");
    }
}

#[test]
fn error_texts_are_chronos() {
    for input in ["", "x", "2023-11-14T22:13:20Zjunk", "2023-13-01T00:00:00Z"] {
        let reference = input
            .parse::<DateTime<Utc>>()
            .expect_err("chrono refuses it");
        let value = input.parse::<Timestamp>().expect_err("we refuse it");
        assert_eq!(value.to_string(), reference.to_string(), "{input:?}");
    }
    let reference = serde_json::from_str::<DateTime<Utc>>("\"x\"").expect_err("refused");
    let value = serde_json::from_str::<Timestamp>("\"x\"").expect_err("refused");
    assert_eq!(value.to_string(), reference.to_string());
    let reference = serde_json::from_str::<DateTime<Utc>>("17").expect_err("refused");
    let value = serde_json::from_str::<Timestamp>("17").expect_err("refused");
    assert_eq!(value.to_string(), reference.to_string());
}

/// jiff has no leap second: `:60` becomes the last nanosecond of `:59`, same unix second.
#[test]
fn a_leap_second_is_clamped() {
    let input = "2016-12-31T23:59:60.5Z";
    let reference = input.parse::<DateTime<Utc>>().expect("chrono reads it");
    let value = input.parse::<Timestamp>().expect("we read it");
    assert_eq!(value.unix_seconds(), reference.timestamp());
    assert_eq!(value.to_string(), "2016-12-31T23:59:59.999999999Z");
    assert_eq!(
        Timestamp::parse_rfc3339("2016-12-31T23:59:60Z").map(ours),
        Ok((1_483_228_799, 999_999_999))
    );
}

/// chrono reached years beyond ±9999, jiff does not: such input is out of range here.
#[test]
fn years_beyond_jiff_are_out_of_range() {
    assert!("+12345-01-01T00:00:00Z".parse::<DateTime<Utc>>().is_ok());
    let error = "+12345-01-01T00:00:00Z"
        .parse::<Timestamp>()
        .expect_err("out of range");
    assert_eq!(error.to_string(), "input is out of range");
}
