//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.

/// The account has no API key stored.
pub(crate) const API_KEY_REQUIRED: (&str, &str) = (
    "premiumize.api_key_required",
    "Premiumize API key is missing",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "premiumize.account_missing",
    "Premiumize account is missing",
);

/// Account label part: the consumed share of the monthly fair-use limit, as `{percent}`.
pub(crate) const FAIR_USE: (&str, &str) = (
    "premiumize.account.fair_use",
    "Fair use: {percent}% consumed",
);

/// `transfer/directdl` returned an empty content list.
pub(crate) const NO_FILE: (&str, &str) = ("premiumize.no_file", "Premiumize did not return a file");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("premiumize.invalid_response", "Invalid Premiumize response");

/// One field of an otherwise well-formed answer has a shape this plugin cannot read; carries
/// the field path as `field`.
pub(crate) const INVALID_RESPONSE_FIELD: (&str, &str) = (
    "premiumize.invalid_response_field",
    "Premiumize sent a field this plugin cannot read",
);

/// The API reported `status != success`; the provider message (if any) is passed through.
pub(crate) const API_ERROR: (&str, &str) = ("premiumize.api_error", "Premiumize API error");

/// Unexpected HTTP status; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "premiumize.http_error";

pub(crate) fn invalid_response_field(field: &str) -> String {
    format!("Premiumize sent a field this plugin cannot read: {field}")
}

pub(crate) fn http_error(status: u16) -> String {
    plugin_common::messages::http_error("Premiumize", status)
}
