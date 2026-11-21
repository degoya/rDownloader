//! The OpenAPI schemas: a derived DTO with the new types renders exactly as one with chrono's.

#![allow(dead_code, reason = "the structs exist for their derived schemas only")]

use utoipa::{PartialSchema, ToSchema};

use super::{Date, Timestamp};

#[derive(ToSchema)]
struct WithChrono {
    at: chrono::DateTime<chrono::Utc>,
    maybe: Option<chrono::DateTime<chrono::Utc>>,
    many: Vec<chrono::DateTime<chrono::Utc>>,
    day: chrono::NaiveDate,
    maybe_day: Option<chrono::NaiveDate>,
}

#[derive(ToSchema)]
struct WithJiff {
    at: Timestamp,
    maybe: Option<Timestamp>,
    many: Vec<Timestamp>,
    day: Date,
    maybe_day: Option<Date>,
}

fn json<T: PartialSchema>() -> serde_json::Value {
    serde_json::to_value(T::schema()).expect("a schema")
}

#[test]
fn a_derived_dto_renders_as_with_chrono() {
    assert_eq!(json::<WithJiff>(), json::<WithChrono>());
}

#[test]
fn the_manual_schemas_are_the_derived_ones() {
    let chrono = json::<WithChrono>();
    assert_eq!(json::<Timestamp>(), chrono["properties"]["at"]);
    assert_eq!(json::<Date>(), chrono["properties"]["day"]);
    assert_eq!(
        json::<Timestamp>(),
        serde_json::json!({"type": "string", "format": "date-time"})
    );
    assert_eq!(
        json::<Date>(),
        serde_json::json!({"type": "string", "format": "date"})
    );
}
