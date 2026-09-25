//! Account label helpers shared by both builds.

use plugin_common::{Label, LabelPart};
use serde::Deserialize;

use crate::messages;

/// `account/info` reports `limit_used` as the consumed fraction (0.0-1.0) of the monthly
/// fair-use limit and never exposes that limit in bytes, so a remaining byte count cannot be
/// derived from it. Documented as a number, so a string is accepted defensively and every
/// other shape is ignored rather than failing the whole account check.
#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct FairUse(serde_json::Value);

impl FairUse {
    fn fraction(&self) -> Option<f64> {
        match &self.0 {
            serde_json::Value::Number(value) => value.as_f64(),
            serde_json::Value::String(value) => value.trim().parse().ok(),
            _ => None,
        }
    }

    /// Whole percent of the fair-use limit; rejects non-finite and negative fractions and caps
    /// an already exceeded limit at 100 so the label never exaggerates.
    fn percent(&self) -> Option<u8> {
        let fraction = self.fraction()?;
        (fraction.is_finite() && fraction >= 0.0).then(|| (fraction.min(1.0) * 100.0).round() as u8)
    }
}

/// Account label: the customer id, extended by the fair-use share when the API reports one.
#[must_use]
pub(crate) fn label(customer_id: Option<String>, fair_use: Option<&FairUse>) -> Label {
    Label::new()
        .user(customer_id.as_deref())
        .maybe(fair_use.and_then(FairUse::percent).map(|percent| {
            LabelPart::coded(
                messages::FAIR_USE.0,
                messages::FAIR_USE
                    .1
                    .replace("{percent}", &percent.to_string()),
            )
            .with_param("percent", percent.to_string())
        }))
}
