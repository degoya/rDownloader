//! The OpenAPI schemas of the time types.
//!
//! A derived `ToSchema` does not call these for a field: utoipa's `chrono` and `jiff_0_2`
//! features make the derive recognise the names `Timestamp` and `Date` and inline the same
//! schema itself. These impls serve `body = Timestamp`, `value_type = Timestamp` and generic
//! positions, and they are what the tests compare with chrono's.

use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, Schema, SchemaFormat, Type};

use super::{Date, Timestamp};

fn string_with(format: KnownFormat) -> RefOr<Schema> {
    RefOr::T(Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(format)))
            .build(),
    ))
}

impl utoipa::PartialSchema for Timestamp {
    fn schema() -> RefOr<Schema> {
        string_with(KnownFormat::DateTime)
    }
}

impl utoipa::ToSchema for Timestamp {}

impl utoipa::PartialSchema for Date {
    fn schema() -> RefOr<Schema> {
        string_with(KnownFormat::Date)
    }
}

impl utoipa::ToSchema for Date {}
