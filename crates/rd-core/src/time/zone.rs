//! `IanaZone`: a named IANA time zone whose every representation is `chrono_tz::Tz`'s.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use jiff::tz::TimeZone;

/// A time zone by its IANA name (`Europe/Berlin`), for the zones rDownloader stores and sends
/// (a schedule's zone) — `chrono_tz::Tz` with the same bytes (RD-1440-05): serde and `Display`
/// are the name, parsing is exact and case-sensitive as chrono-tz's was. The zone data come
/// from jiff (the system's `zoneinfo`, else the bundled copy). Equality and hashing go by name.
/// OpenAPI: keep the field's `#[schema(value_type = String)]`.
#[derive(Clone)]
pub struct IanaZone(TimeZone);

/// A name that is no IANA zone; its text is chrono-tz's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnknownZone;

impl fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("failed to parse timezone")
    }
}

impl std::error::Error for UnknownZone {}

impl IanaZone {
    /// `UTC`.
    #[must_use]
    pub const fn utc() -> Self {
        Self(TimeZone::UTC)
    }

    /// The zone called exactly `name`.
    ///
    /// # Errors
    /// When the zone database knows no zone of that name, in that spelling.
    pub fn parse(name: &str) -> Result<Self, UnknownZone> {
        if name == "UTC" {
            return Ok(Self::utc());
        }
        let zone = TimeZone::get(name).map_err(|_| UnknownZone)?;
        // jiff looks names up case-insensitively; chrono-tz did not.
        if zone.iana_name() == Some(name) {
            Ok(Self(zone))
        } else {
            Err(UnknownZone)
        }
    }

    /// The IANA name, as stored and sent.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.iana_name().unwrap_or("UTC")
    }

    /// The jiff zone, for `Timestamp::to_zoned_datetime` and civil arithmetic.
    #[must_use]
    pub fn as_jiff(&self) -> &TimeZone {
        &self.0
    }
}

impl Default for IanaZone {
    fn default() -> Self {
        Self::utc()
    }
}

impl PartialEq for IanaZone {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for IanaZone {}

impl Hash for IanaZone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name().hash(state);
    }
}

impl fmt::Display for IanaZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Debug for IanaZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for IanaZone {
    type Err = UnknownZone;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl From<IanaZone> for TimeZone {
    fn from(zone: IanaZone) -> Self {
        zone.0
    }
}

impl serde::Serialize for IanaZone {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> serde::Deserialize<'de> for IanaZone {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = IanaZone;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an IANA timezone string")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<IanaZone, E> {
                IanaZone::parse(value)
                    .map_err(|_| E::custom(format_args!("failed to parse timezone: '{value}'")))
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}
