//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once, and `locales/` translates exactly these.
//! Nothing pCloud wrote appears in any of them: an API answer carries an `error` written as an
//! English sentence for a developer, and repeating it would put a provider's prose — and
//! whatever it happened to quote — into a log line and into the interface. What travels
//! instead is `result`, pCloud's own decimal refusal number, which cannot carry anything.
#![allow(dead_code)] // The native tests and the guest use different subsets.

/// Which of pCloud's two installations the account lives in. Not a failure: it stands on the
/// account row, because every region mistake reads like a bad credential until somebody can
/// see this.
pub(crate) const ACCOUNT_REGION: (&str, &str) =
    ("pcloud.account_region", "pCloud data centre: {region}");

/// The request carried no account identity, so there is nothing to sign the call with.
pub(crate) const ACCOUNT_MISSING: (&str, &str) =
    ("pcloud.account_missing", "pCloud account is missing");

/// No token is stored for this account, or neither pCloud installation accepted the one that
/// is.
pub(crate) const SIGN_IN_REQUIRED: (&str, &str) = (
    "pcloud.sign_in_required",
    "This pCloud account has to be signed in again",
);

/// The address is not a pCloud file address.
pub(crate) const NOT_A_PCLOUD_LINK: (&str, &str) = (
    "pcloud.not_a_pcloud_link",
    "This is not a pCloud file address",
);

/// The file is gone, or the account cannot see it — in either installation.
pub(crate) const FILE_NOT_FOUND: (&str, &str) = (
    "pcloud.file_not_found",
    "This pCloud file could not be found",
);

/// The address names a folder, which the folder crawler lists rather than the resolver.
pub(crate) const IS_A_FOLDER: (&str, &str) =
    ("pcloud.is_a_folder", "This pCloud address is a folder");

/// pCloud will not serve these bytes to this account.
pub(crate) const DOWNLOAD_NOT_PERMITTED: (&str, &str) = (
    "pcloud.download_not_permitted",
    "pCloud does not allow this file to be downloaded",
);

/// A public link pCloud refused: gone, expired, out of traffic, or password-protected. The
/// `result` parameter is pCloud's own number for which of those it was.
pub(crate) const LINK_UNAVAILABLE: (&str, &str) = (
    "pcloud.link_unavailable",
    "pCloud will not open this public link (result {result})",
);

/// pCloud is rate limiting this application or this address.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "pcloud.rate_limited",
    "pCloud is rate limiting this account",
);

/// pCloud answered something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("pcloud.invalid_response", "Invalid pCloud response");

/// pCloud named a download host that is not one of its own, so nothing was handed on.
pub(crate) const INVALID_DOWNLOAD_HOST: (&str, &str) = (
    "pcloud.invalid_download_host",
    "pCloud answered with a download address that is not pCloud's",
);

/// pCloud is away, or answered one of its 5xxx results.
pub(crate) const UNAVAILABLE: (&str, &str) =
    ("pcloud.unavailable", "pCloud is temporarily unavailable");

/// A refusal pCloud numbered that this plugin has no case for. The number travels as the
/// `result` parameter — a decimal integer, never free text.
pub(crate) const API_REFUSED: (&str, &str) = (
    "pcloud.api_refused",
    "pCloud refused this request (result {result})",
);
