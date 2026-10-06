//! A count the provider sends as a JSON number on one endpoint and as a numeric string on
//! another.
//!
//! The same two-armed `#[serde(untagged)]` enum lived eleven times over -- box, onedrive,
//! google-drive and their crawlers, premiumize twice, nitroflare, 1fichier and `xfs-common` --
//! each with its own `as_u64` or `into_u64` (RD-1120-10). It is a crate of its own rather than a
//! feature of `plugin-common`, which depends on nothing under `wasm32` on purpose; every plugin
//! that reads one of these already links `serde`, so a component gains no dependency.
//!
//! Two shapes, because the copies accepted two: [`FlexibleU64`] refuses a fractional number (the
//! answer fails to parse, as it always did), [`LenientU64`] takes one and truncates it, which is
//! what premiumize's answers need.

#![forbid(unsafe_code)]

use serde::Deserialize;

/// A whole number sent as a JSON number or as a numeric string. A string that is not a whole
/// number reads as no value; a fractional or negative JSON number does not deserialize at all.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum FlexibleU64 {
    Number(u64),
    Text(String),
}

impl FlexibleU64 {
    /// The value, or `None` for a string that is not a whole number.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }

    /// [`Self::as_u64`] for a caller that owns the value, e.g. in `Option::and_then`.
    #[must_use]
    pub fn into_u64(self) -> Option<u64> {
        self.as_u64()
    }
}

/// [`FlexibleU64`] that also takes a fractional JSON number, truncated toward zero; a negative
/// one reads as no value.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum LenientU64 {
    Number(u64),
    Float(f64),
    Text(String),
}

impl LenientU64 {
    /// The value, or `None` for a negative number or a string that is not a whole number.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Self::Float(value) if *value >= 0.0 => Some(*value as u64),
            Self::Float(_) => None,
            Self::Text(value) => value.parse().ok(),
        }
    }

    /// [`Self::as_u64`] for a caller that owns the value.
    #[must_use]
    pub fn into_u64(self) -> Option<u64> {
        self.as_u64()
    }
}

#[cfg(test)]
mod tests {
    use super::{FlexibleU64, LenientU64};

    #[test]
    fn flexible_reads_a_number_and_a_numeric_string() {
        let number: FlexibleU64 = serde_json::from_str("42").expect("number");
        assert_eq!(number.as_u64(), Some(42));
        let text: FlexibleU64 = serde_json::from_str("\"4096\"").expect("text");
        assert_eq!(text.into_u64(), Some(4096));
    }

    #[test]
    fn flexible_reads_a_non_numeric_string_as_no_value_and_refuses_a_fraction() {
        let text: FlexibleU64 = serde_json::from_str("\"12 MB\"").expect("text");
        assert_eq!(text.as_u64(), None);
        assert!(serde_json::from_str::<FlexibleU64>("1.5").is_err());
        assert!(serde_json::from_str::<FlexibleU64>("-1").is_err());
        assert!(serde_json::from_str::<FlexibleU64>("null").is_err());
    }

    #[test]
    fn lenient_truncates_a_fraction_and_drops_a_negative_one() {
        let float: LenientU64 = serde_json::from_str("1536.9").expect("float");
        assert_eq!(float.as_u64(), Some(1536));
        let negative: LenientU64 = serde_json::from_str("-1").expect("negative");
        assert_eq!(negative.into_u64(), None);
        let text: LenientU64 = serde_json::from_str("\"77\"").expect("text");
        assert_eq!(text.as_u64(), Some(77));
        let number: LenientU64 = serde_json::from_str("9").expect("number");
        assert_eq!(number.as_u64(), Some(9));
    }
}
