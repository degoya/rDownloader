//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.
#![allow(dead_code)] // Native and guest adapters use different subsets of these constants.

/// The account has no Debrid-Link API key configured.
pub(crate) const API_KEY_MISSING: (&str, &str) = (
    "debridlink.api_key_missing",
    "Debrid-Link API key is missing",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "debridlink.account_missing",
    "Debrid-Link account is missing",
);

/// Debrid-Link's `requestFileInformation` reports `AvailableStatus.UNCHECKABLE` (JD never queries
/// a batch link-status endpoint), so `check()` is not implemented. The native `Resolver` trait
/// (`rd_plugin_api::Resolver::check`) has a default that reports exactly this `(code, message)`
/// pair; the WIT `Guest` trait has no such default (every export must be implemented), so
/// `guest.rs` reproduces it explicitly via this constant instead of a hand-copied literal that
/// could drift from the trait default. `native/tests.rs::check_defaults_to_unsupported` asserts
/// the native adapter's (trait-default) output equals this constant, which keeps the two in sync.
pub(crate) const CHECK_UNSUPPORTED: (&str, &str) = (
    "link.check_unsupported",
    "Link check is not supported by this resolver",
);

/// API responded `badToken`, or HTTP 401/403. The JSON-error path attaches the provider's raw
/// error key as an `api_code` param (see `api::coded_with_provider_code`); the bare HTTP-status
/// path (401/403 with no JSON envelope to read a key from) does not.
pub(crate) const AUTH_INVALID: (&str, &str) = (
    "debridlink.auth_invalid",
    "Debrid-Link API key is invalid or expired",
);

/// API responded `fileNotFound`, or HTTP 404/410/451.
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "debridlink.file_offline",
    "Debrid-Link file is not available",
);

/// API responded `notDebrid`/`hostNotValid`/`notFreeHost`: JD's `downloadErrorsHostUnavailable`
/// bucket, handled with a 5-minute link retry (`MultiHosterManagement#putError`), not a permanent
/// capability gap — see `api.rs`'s IMPL-VERIFY note on why this is `Transient`, not `Unsupported`.
pub(crate) const HOST_UNSUPPORTED: (&str, &str) = (
    "debridlink.host_unsupported",
    "Debrid-Link cannot currently generate a link for this host",
);

/// API responded `freeServerOverload`/`serverNotAllowed`/`maintenanceHost`/`noServerHost`/
/// `disabledServerHost`/`accountLocked`: JD's remaining `downloadErrorsHostUnavailable` and
/// `accountErrorsTemporary` members, all handled with the same 5-minute retry.
pub(crate) const SERVER_BUSY: (&str, &str) = (
    "debridlink.server_busy",
    "Debrid-Link server is temporarily busy",
);

/// API responded `maxLink`/`maxLinkHost`/`maxData`/`maxDataHost`: a daily quota was reached.
pub(crate) const LIMIT_REACHED: (&str, &str) = (
    "debridlink.limit_reached",
    "Debrid-Link daily limit was reached",
);

/// API responded `floodDetected`.
pub(crate) const FLOOD: (&str, &str) = (
    "debridlink.flood",
    "Debrid-Link API rate limit was triggered",
);

/// `downloader/add` reported no error but omitted `value.downloadUrl`.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "debridlink.no_download_url",
    "Debrid-Link did not return a download URL",
);

/// HTTP 429, not covered by a JSON error envelope.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "debridlink.rate_limited",
    "Debrid-Link API rate limit was triggered",
);

/// HTTP 5xx or a network-level failure.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("debridlink.server_error", "Debrid-Link server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) = (
    "debridlink.invalid_response",
    "Invalid Debrid-Link response",
);

/// The API envelope reported an error key not covered by a specific code above; carries an
/// `api_code` parameter with the raw key.
pub(crate) const API_ERROR: &str = "debridlink.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "debridlink.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "debridlink.invalid_url";

pub(crate) fn api_error(api_code: &str) -> String {
    format!("Debrid-Link API error: {api_code}")
}

pub(crate) fn http_error(status: u16) -> String {
    format!("Debrid-Link HTTP status {status}")
}

pub(crate) fn invalid_url(error: &dyn std::fmt::Display) -> String {
    format!("Invalid provider URL: {error}")
}
