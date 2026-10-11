//! User-facing texts and stable failure codes shared by the native and WebAssembly adapters.
//!
//! Each `(code, message)` pair exists exactly once so both targets report identical text.

/// The account has no LinkSnappy password configured.
pub(crate) const PASSWORD_MISSING: (&str, &str) = (
    "linksnappy.password_missing",
    "LinkSnappy password is missing",
);

/// The request carried no account identity.
pub(crate) const ACCOUNT_MISSING: (&str, &str) = (
    "linksnappy.account_missing",
    "LinkSnappy account is missing",
);

/// `link.check_unsupported`, reproduced verbatim from the native `Resolver` trait's default
/// `check()` (`rd_plugin_api::Resolver::check`) — LinkSnappy has no batch link-status endpoint
/// (availability is only checked one link at a time via the same `linkgen` call `resolve()` uses),
/// so `check()` is left at the trait default on the native side. The WIT `Guest` trait has no such
/// default (every export must be implemented), so `guest.rs` reproduces it explicitly via this
/// constant instead of a hand-copied literal that could drift.
/// `native/tests.rs::check_defaults_to_unsupported` asserts the native adapter's (trait-default)
/// output equals this constant.
pub(crate) const CHECK_UNSUPPORTED: (&str, &str) = (
    "link.check_unsupported",
    "Link check is not supported by this resolver",
);

/// The API reported "Incorrect Username or Password" — the only message that means the account
/// itself is invalid.
pub(crate) const BAD_CREDENTIALS: (&str, &str) = (
    "linksnappy.bad_credentials",
    "LinkSnappy username or password is wrong",
);

/// The API reported "Two-Factor Verification Required" — the account's first API login
/// needs e-mail confirmation before it can be used. It is a temporary block (5 minutes, like the
/// daily-quota/elite-membership errors below), not an invalid account, so this is mapped
/// `RateLimited` rather than `AccountInvalid`: nothing here can tell "confirmation pending" from
/// "confirmed" ahead of time, and the account may already work again once the user visits the
/// confirmation link the site's e-mail carries.
pub(crate) const TWO_FACTOR_REQUIRED: (&str, &str) = (
    "linksnappy.two_factor_required",
    "LinkSnappy requires confirming this login via the e-mail sent to the account",
);

/// The API reported "No server available for this filehost, Please retry after few
/// minutes" — a per-host backend capacity problem, not an account-level condition: only the
/// affected host waits 5 minutes, the whole account does not fail.
pub(crate) const HOST_UNAVAILABLE: (&str, &str) = (
    "linksnappy.host_unavailable",
    "LinkSnappy has no server available for this file host right now",
);

/// The API reported "You have reached max download request" — too many link-generation
/// requests in a short time (distinct from the daily traffic/link quota below); the affected
/// host waits 5 minutes.
pub(crate) const TOO_MANY_REQUESTS: (&str, &str) = (
    "linksnappy.too_many_requests",
    "LinkSnappy rejected the request; too many recent download requests",
);

/// The API reported "You have reached max download limit of ..." (1-minute retry) or
/// "Account has exceeded the daily quota" (5-minute retry) — both are daily traffic/link cap
/// signals; the retry delay is picked per exact matched phrase (see `api::classify_message`),
/// but both share this code since the underlying condition (daily limit reached) is the same.
pub(crate) const LIMIT_REACHED: (&str, &str) = (
    "linksnappy.limit_reached",
    "LinkSnappy traffic or download limit was reached",
);

/// The API reported "Invalid file URL format." — this must **not** disable the host: the
/// *link's* format is not recognized by LinkSnappy, not the host in general. It is a temporary
/// condition with no explicit wait (default scheduler backoff), so this is `Transient` with no
/// fixed `retry_after_seconds`, not `Unsupported` (which would tell the scheduler this resolver can
/// never handle the link at all).
pub(crate) const INVALID_LINK_FORMAT: (&str, &str) = (
    "linksnappy.invalid_link_format",
    "LinkSnappy does not recognize this link's format",
);

/// The API reported "File not found" or "File deleted on ...".
pub(crate) const FILE_OFFLINE: (&str, &str) = (
    "linksnappy.file_offline",
    "LinkSnappy file is not available",
);

/// The API reported "Your Account has Expired" — a 5-minute temporary block, not an invalid
/// account; mapped `RateLimited` for the same reason as [`TWO_FACTOR_REQUIRED`], and deliberately
/// **not** the `AuthRequired`/`premium_required` kind the task brief first suggested for
/// "expired" accounts (see `api.rs`'s module-level IMPL-VERIFY note).
pub(crate) const ACCOUNT_EXPIRED: (&str, &str) = (
    "linksnappy.account_expired",
    "LinkSnappy account has expired",
);

/// The API reported exactly "This file requires password" (an exact-string match, not a
/// substring search). This plugin has no way to collect or resend a per-file download password
/// (`ResolveRequest` carries only a URL), so rather than a retry with the user's input this is
/// reported as a hard `Permanent` failure.
pub(crate) const PASSWORD_PROTECTED: (&str, &str) = (
    "linksnappy.password_protected",
    "LinkSnappy file requires a download password, which this plugin cannot supply",
);

/// The API reported "Please upgrade to Elite membership" — the free account's daily
/// link-count cap. Kept under the task brief's `premium_required` name (the message is genuinely
/// about needing an Elite/premium plan), but mapped `RateLimited{600}`, a 10-minute temporary
/// account block, not the brief's suggested `AuthRequired` — see `api.rs`'s IMPL-VERIFY note.
pub(crate) const PREMIUM_REQUIRED: (&str, &str) = (
    "linksnappy.premium_required",
    "LinkSnappy Elite (premium) membership is required to use the download API",
);

/// Message contains "not supported" (case-insensitive). **Not** a known API message — no
/// generic "host not supported" error string was found in this plugin's IMPL-VERIFY pass. Included
/// only because the task brief explicitly asks for this mapping; kept as a defensive, best-effort
/// substring match so a future wording change on LinkSnappy's side is still caught by something
/// other than the generic [`API_ERROR`] bucket. Unverified against any known LinkSnappy response.
pub(crate) const HOST_UNSUPPORTED: (&str, &str) = (
    "linksnappy.host_unsupported",
    "LinkSnappy does not support this file host",
);

/// `genLinks` reported no error but omitted `links` entirely, or the one `links[0]` entry it did
/// return omitted `generated` — both root causes are one temporary condition ("Failed to find
/// final downloadurl") with no explicit wait (default scheduler backoff), hence `Transient` here
/// rather than the `Permanent` other multihoster plugins in this workspace use for a missing
/// download URL.
pub(crate) const NO_DOWNLOAD_URL: (&str, &str) = (
    "linksnappy.no_download_url",
    "LinkSnappy did not return a download URL",
);

/// HTTP 429, not covered by a JSON error envelope.
pub(crate) const RATE_LIMITED: (&str, &str) = (
    "linksnappy.rate_limited",
    "LinkSnappy API rate limit was triggered",
);

/// HTTP 5xx, or a bare HTTP 425 ("still preparing this transfer", the final download stream's
/// code, reused here defensively for the JSON API), not covered by
/// a JSON error envelope.
pub(crate) const SERVER_ERROR: (&str, &str) =
    ("linksnappy.server_error", "LinkSnappy server error");

/// The API answered with something that is not the expected JSON.
pub(crate) const INVALID_RESPONSE: (&str, &str) =
    ("linksnappy.invalid_response", "Invalid LinkSnappy response");

/// The API reported an error not covered by a specific code above; carries the provider
/// `message`. The fallback treats an account-level error (no link in scope —
/// `check_account`/`hosters()` in this plugin) as a 10-minute temporary account block, and a
/// link-level error (`resolve()`) as a 5-minute retry on that one link; both carry an explicit
/// wait, neither is a permanent failure.
pub(crate) const API_ERROR: &str = "linksnappy.api_error";

/// Unexpected HTTP status not covered by a specific code above; carries a `status` parameter.
pub(crate) const HTTP_ERROR: &str = "linksnappy.http_error";

/// A provider URL failed to parse; carries the parser `error`.
pub(crate) const INVALID_URL: &str = "linksnappy.invalid_url";

pub(crate) fn api_error(message: &str) -> String {
    format!("LinkSnappy API error: {message}")
}

pub(crate) fn http_error(status: u16) -> String {
    plugin_common::messages::http_error("LinkSnappy", status)
}
