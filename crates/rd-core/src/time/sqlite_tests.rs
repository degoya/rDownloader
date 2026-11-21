//! The SQLite impls against `sqlx`'s own chrono impls, on a real in-memory database.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Connection, SqliteConnection};

use super::{Date, Timestamp};

fn ours(value: Timestamp) -> (i64, u32) {
    (value.unix_seconds(), value.subsec_nanos())
}

fn theirs(value: DateTime<Utc>) -> (i64, u32) {
    (value.timestamp(), value.timestamp_subsec_nanos())
}

async fn connection() -> SqliteConnection {
    SqliteConnection::connect("sqlite::memory:")
        .await
        .expect("an in-memory database")
}

const INSTANTS: &[(i64, u32)] = &[
    (0, 0),
    (1_700_000_000, 0),
    (1_700_000_000, 5),
    (1_700_000_000, 120_000_000),
    (1_700_000_000, 123_456_000),
    (1_700_000_000, 123_456_789),
    (1_700_000_000, 999_999_999),
    (-1, 500_000_000),
    (-62_167_219_200, 0),
];

#[tokio::test]
async fn encodes_the_text_sqlx_chrono_writes() {
    let mut db = connection().await;
    for &(seconds, nanos) in INSTANTS {
        let value = Timestamp::from_unix(seconds, nanos).expect("in range");
        let reference = DateTime::<Utc>::from_timestamp(seconds, nanos).expect("in range");
        let written: String = sqlx::query_scalar("SELECT ?1")
            .bind(value)
            .fetch_one(&mut db)
            .await
            .expect("our text");
        let expected: String = sqlx::query_scalar("SELECT ?1")
            .bind(reference)
            .fetch_one(&mut db)
            .await
            .expect("chrono's text");
        assert_eq!(written, expected);
        assert!(written.ends_with("+00:00"), "{written}");
        let back: Timestamp = sqlx::query_scalar("SELECT ?1")
            .bind(value)
            .fetch_one(&mut db)
            .await
            .expect("decoded");
        assert_eq!(back, value);
    }
}

#[tokio::test]
async fn decodes_every_text_form_sqlx_chrono_reads() {
    let mut db = connection().await;
    let inputs = [
        "2023-11-14T22:13:20Z",
        "2023-11-14T22:13:20+00:00",
        "2023-11-14T22:13:20.123+00:00",
        "2023-11-14T22:13:20.123456789+02:00",
        "2023-11-14 22:13:20",
        "2023-11-14 22:13:20.5",
        "2023-11-14 22:13",
        "2023-11-14 22:13Z",
        "2023-11-14 22:13+01:00",
        "2023-11-14 22:13:20Z",
        "2023-11-14 22:13:20.25+01:00",
        "2023-11-14T22:13",
        "2023-11-14T22:13Z",
        "2023-11-14T22:13-03:00",
        "2023-11-14T22:13:20",
        "2023-11-14T22:13:20.75Z",
        "2023-11-14T22:13:20.75-01:00",
        "",
        "yesterday",
        "2023-11-14",
        "2023-11-14T22",
    ];
    for input in inputs {
        let reference: Result<DateTime<Utc>, _> = sqlx::query_scalar("SELECT ?1")
            .bind(input)
            .fetch_one(&mut db)
            .await;
        let value: Result<Timestamp, _> = sqlx::query_scalar("SELECT ?1")
            .bind(input)
            .fetch_one(&mut db)
            .await;
        assert_eq!(
            value.ok().map(ours),
            reference.ok().map(theirs),
            "{input:?}"
        );
    }
    // SQLite's own clock texts, as the migrations' defaults write them.
    for sql in [
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        "SELECT datetime('now')",
        "SELECT strftime('%Y-%m-%d %H:%M:%f', 'now')",
    ] {
        let reference: DateTime<Utc> = sqlx::query_scalar(sql)
            .fetch_one(&mut db)
            .await
            .expect("chrono reads it");
        let value: Timestamp = sqlx::query_scalar(sql)
            .fetch_one(&mut db)
            .await
            .expect("we read it");
        // Two calls to 'now' within one statement each; the second may tick on.
        assert!(
            (value.unix_seconds() - reference.timestamp()).abs() <= 1,
            "{sql}"
        );
    }
}

#[tokio::test]
async fn decodes_integers_and_julian_days_as_sqlx_chrono_does() {
    let mut db = connection().await;
    for seconds in [0i64, 1_700_000_000, -1, -86_400 * 365] {
        let reference: DateTime<Utc> = sqlx::query_scalar("SELECT ?1")
            .bind(seconds)
            .fetch_one(&mut db)
            .await
            .expect("chrono reads it");
        let value: Timestamp = sqlx::query_scalar("SELECT ?1")
            .bind(seconds)
            .fetch_one(&mut db)
            .await
            .expect("we read it");
        assert_eq!(ours(value), theirs(reference), "{seconds}");
    }
    for text in [
        "2023-11-14 22:13:20.5",
        "1969-12-31 23:59:59.5",
        "1970-01-01 00:00:01.5",
    ] {
        let reference: DateTime<Utc> = sqlx::query_scalar("SELECT julianday(?1)")
            .bind(text)
            .fetch_one(&mut db)
            .await
            .expect("chrono reads it");
        let value: Timestamp = sqlx::query_scalar("SELECT julianday(?1)")
            .bind(text)
            .fetch_one(&mut db)
            .await
            .expect("we read it");
        assert_eq!(ours(value), theirs(reference), "{text}");
    }
}

#[tokio::test]
async fn null_is_none_both_ways() {
    let mut db = connection().await;
    let value: Option<Timestamp> = sqlx::query_scalar("SELECT ?1")
        .bind(None::<Timestamp>)
        .fetch_one(&mut db)
        .await
        .expect("a NULL");
    assert_eq!(value, None);
    let date: Option<Date> = sqlx::query_scalar("SELECT ?1")
        .bind(None::<Date>)
        .fetch_one(&mut db)
        .await
        .expect("a NULL");
    assert_eq!(date, None);
    let some: Option<Timestamp> = sqlx::query_scalar("SELECT ?1")
        .bind(Some(Timestamp::UNIX_EPOCH))
        .fetch_one(&mut db)
        .await
        .expect("a value");
    assert_eq!(some, Some(Timestamp::UNIX_EPOCH));
}

/// `ORDER BY` on a column of ours, rows of chrono's text among them, orders the instants.
#[tokio::test]
async fn order_by_orders_the_instants() {
    let mut db = connection().await;
    sqlx::query("CREATE TABLE t (at TEXT NOT NULL)")
        .execute(&mut db)
        .await
        .expect("a table");
    let mut values = Vec::new();
    for (index, &(seconds, nanos)) in INSTANTS.iter().rev().enumerate() {
        let value = Timestamp::from_unix(seconds, nanos).expect("in range");
        let insert = sqlx::query("INSERT INTO t (at) VALUES (?1)");
        let insert = if index % 2 == 0 {
            insert.bind(value)
        } else {
            insert.bind(DateTime::<Utc>::from_timestamp(seconds, nanos).expect("in range"))
        };
        insert.execute(&mut db).await.expect("a row");
        values.push(value);
    }
    values.sort();
    let ordered: Vec<Timestamp> = sqlx::query_scalar("SELECT at FROM t ORDER BY at")
        .fetch_all(&mut db)
        .await
        .expect("the rows");
    // Four-digit years (0000..=9999) keep the text order the instant order.
    assert_eq!(ordered, values);
}

#[tokio::test]
async fn dates_are_the_text_chrono_writes() {
    let mut db = connection().await;
    let value = Date::from_ymd(2026, 3, 9).expect("a date");
    let reference = NaiveDate::from_ymd_opt(2026, 3, 9).expect("a date");
    let written: String = sqlx::query_scalar("SELECT ?1")
        .bind(value)
        .fetch_one(&mut db)
        .await
        .expect("our text");
    let expected: String = sqlx::query_scalar("SELECT ?1")
        .bind(reference)
        .fetch_one(&mut db)
        .await
        .expect("chrono's text");
    assert_eq!(written, expected);
    let back: Date = sqlx::query_scalar("SELECT ?1")
        .bind("2026-03-09")
        .fetch_one(&mut db)
        .await
        .expect("decoded");
    assert_eq!(back, value);
}
