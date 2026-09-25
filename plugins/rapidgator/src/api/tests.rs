//! Unit tests for [`super`], and the home of the full IMPL-VERIFY provenance for
//! `crate::api`'s error classification (moved out of `api.rs`'s module doc to keep that file
//! under the workspace's 500-line limit — see plugin-common.md).
//!
//! IMPL-VERIFY (against JD's `RapidGatorNet.java`, the living reference —
//! `svn_trunk/src/jd/plugins/hoster/RapidGatorNet.java`, revision 53165):
//! - Endpoints/params confirmed verbatim: `user/login?login=<email>&password=<pw>`,
//!   `file/info?token=<t>&file_id=<id>`, `file/download?token=<t>&file_id=<id>` — `file_id`, not
//!   `url`, as the brief's "VERIFY" flagged. `user/info?token=<t>` also exists but JD only calls
//!   it to validate a *cached* session (`loginAPI`'s session-reuse branch); the account-info flow
//!   this plugin implements (`fetchAccountInfoAPI` → `loginAPI` → `parseAPIAccountInfo`) reads
//!   `user`/`storage` straight from `user/login`'s own response. Since this plugin logs in fresh
//!   on every invocation (no session cache — WASM is stateless, per the brief), it never needs
//!   `user/info` at all: `check_account` uses `user/login`'s response directly. Deviates from the
//!   brief's suggested "login → separate `user/info` call" two-step; JD wins.
//! - The file-id pattern (`getFID`: `(?i)/file/([a-z0-9]{32}|\d+)`) is case-insensitive over
//!   `[a-z0-9]`, i.e. any 32 *alphanumeric* ASCII characters (not hex-restricted, despite the
//!   brief's "32-hex-or-numeric-id" phrasing) or one-or-more digits, immediately after `/file/`.
//!   The URL-recognition pattern additionally allows an optional trailing `/name.html` segment,
//!   which is naturally handled here by taking only the first path segment after `/file/`.
//! - Response envelope confirmed as `{"response": T|null, "status": <int>, "details": <string>}`
//!   (JD's 2019-12-16+ shape, e.g. `{"response":null,"status":423,"details":"Error: Exceeded
//!   traffic"}`); `status` absent or `200` is success. JD also tolerates an older
//!   `{"error":"...","success":false}` shape as a fallback via `response_details`/`error` key
//!   lookups — not implemented here since it predates the living reference's primary path and
//!   the task brief's own envelope description matches the confirmed current shape.
//! - `handleErrors_api`'s classification (the complete list, in JD's actual precedence order —
//!   phrase-based checks run first, numeric `status` fallbacks only for what phrases don't
//!   catch — see [`super::classify_status`] for the mirrored order):
//!   - `status == 423` OR message contains "Exceeded traffic" → traffic/storage limit reached.
//!     JD's retry delay is **5 minutes** (`5 * 60 * 1000`), not the brief's proposed 3600s.
//!   - message contains "Denied by IP" → JD throws `AccountUnavailableException` (a *temporary*
//!     block, 2 hours), **not** `AccountInvalidException` as the brief assumed; mapped to
//!     `RateLimited{7200}` here rather than `AccountInvalid`.
//!   - message contains "Please wait" → login flood protection; JD's account-check-context delay
//!     is 5 minutes (a link-context/`resolve` variant exists too, at 10 minutes — not separately
//!     replicated since this module has one shared classifier for both call sites).
//!   - message contains "User is not PREMIUM" / "This file can be downloaded by premium only" /
//!     "You can download files up to" → account lacks premium (`AccountRequiredException`).
//!   - message contains "Login or password is wrong" / "Error: Error e-mail or password" /
//!     "Password cannot be blank" / "User is FROZEN" / "Error: ACCOUNT LOCKED FOR VIOLATION OF
//!     OUR TERMS. PLEASE CONTACT SUPPORT." / "Parameter login or password is missing", or
//!     `status == 401` with "Wrong e-mail or password" → invalid credentials
//!     (`AccountInvalidException`).
//!   - `status == 401` (otherwise unmatched) or message mentions "Session not exist"/"Session
//!     doesn't exist"/"This download session is not for you"/"Session not found" →
//!     session-invalid. JD's handling here (`handleInvalidSession`) exists entirely to manage a
//!     *cached* session's lifecycle; this plugin never caches a session, so this arm should never
//!     fire in practice and is mapped to a bare `Transient` retry rather than replicating JD's
//!     stateful retry-tracking map.
//!   - `status == 404` → **not** uniform. JD calls `handleErrors_api(..., trustError404)` with a
//!     per-endpoint flag: `requestFileInformationAPI` (`file/info`) passes `true` and a 404 there
//!     is trusted unconditionally as offline. `handlePremium_api` (`file/download`) passes
//!     `false` — JD's own comment explains why, and it is an **API-bug workaround, not a
//!     session-caching concern** (an earlier version of this file's doc comment mischaracterized
//!     it as the latter): "Rapidgator API IN SOME SITUATIONS has a bug which will return invalid
//!     offline status. Do NOT trust this status anymore!" (JD ~lines 1832-1846). JD's untrusted
//!     path ultimately retries via `handleInvalidSession` → `throwAccountUnavailableException`, a
//!     60-second wait, rather than declaring the file offline — so a 404 on `file/download`
//!     (which can happen even right after `file/info` just confirmed the file online, exactly
//!     JD's documented bug scenario) must not be treated as permanently offline here either.
//!     Threaded through as [`super::classify_status`]/[`super::ensure_http_status`]'s `trust_404`
//!     parameter: `true` for `file/info`, `false` for `file/download`.
//!   - `status == 500` → transient, JD's envelope-level retry is 5 minutes.
//!   - `status == 503` → transient, JD's envelope-level retry is 30 minutes.
//!   - message contains "Error: You requested login to your account from unusual Ip address" →
//!     JD throws `AccountUnavailableException(msg, 60_000)` — the same temporary-block exception
//!     class as "Denied by IP" and the traffic-limit branch above, so this is mapped
//!     `RateLimited{60}` here too (an earlier version of this file mapped it to `AccountInvalid`,
//!     which does not match JD's exception class).
//!   - anything else (JD's final `else` arm) → **transient**, retry after 60 seconds
//!     (`PluginException(ERROR_TEMPORARILY_UNAVAILABLE, ..., 60_000)` /
//!     `AccountUnavailableException(..., 60_000)`) — deliberately not `Permanent`, unlike the
//!     sibling nitroflare/1fichier plugins' own "unknown code" convention, because JD's own
//!     fallback here is explicitly retryable.
//! - Bare (non-JSON-classified) HTTP statuses, from JD's `createNewBrowserInstance`
//!   (`addAllowedResponseCodes(401, 402, 501, 423)`) and `handleErrors_api`'s upfront
//!   `con.getResponseCode()` checks: `401` → invalid credentials (immediate, no body parsed);
//!   `404` → offline or untrusted-retry, same `trust_404` split as above (JD's bare-status check
//!   also calls `handle404API(..., trustError404)`); `416` → transient, 5 minutes; `423` → falls
//!   through to JSON parsing (not thrown directly — mirrored here by attempting the envelope
//!   parse before falling back to [`super::ensure_http_status`]); `500` → transient, **60
//!   minutes** (JD's bare-HTTP-500 delay is much longer than its envelope-level-500 delay above);
//!   `503` → transient, 5 minutes. `429` is not a code JD's plugin handles (Rapidgator's API
//!   doesn't appear to use it) but is kept as a defensive fallback for parity with the other
//!   plugins in this workspace.
//! - `file/info`'s `response.file` object exposes `name`/`size`/`hash` (an MD5 hex digest); no
//!   plugin in this workspace currently surfaces a resolved-download checksum (all set `None`),
//!   so this plugin follows that convention rather than introducing new checksum plumbing.
//! - `user/login`'s `response.user` object exposes `is_premium` (bool), `premium_end_time` (an
//!   epoch-seconds timestamp, present only for premium accounts — JD adds a 24h grace buffer when
//!   setting the visible expiry, not replicated here since this plugin doesn't persist account
//!   state) and `traffic.left`/`traffic.total` (bytes; `null` for a free account, which JD reports
//!   as unlimited traffic — mirrored by `traffic_left: None` for a non-premium account here).
//! - No CDN/premium-download host allowlist is hardcoded anywhere in JD's plugin (no domain
//!   validation is applied to `download_url`); `manifest.toml`'s `download_domains` keeps the
//!   brief's wildcarded `*.rapidgator.net`, which covers any subdomain the API might return.

use super::*;

fn url(value: &str) -> Url {
    value.parse().expect("URL")
}

#[test]
fn file_id_accepts_32_char_and_numeric_ids_across_hosts() {
    let id32 = "aBc123XYZ0aBc123XYZ0aBc123XYZ0aB";
    assert_eq!(id32.len(), 32);
    assert_eq!(
        file_id(&url(&format!("https://rapidgator.net/file/{id32}"))),
        Some(id32)
    );
    assert_eq!(
        file_id(&url(&format!("https://www.rapidgator.net/file/{id32}"))),
        Some(id32)
    );
    assert_eq!(file_id(&url("https://rg.to/file/123456")), Some("123456"));
    assert_eq!(
        file_id(&url("https://rapidgator.asia/file/123456")),
        Some("123456")
    );
}

#[test]
fn file_id_tolerates_a_trailing_name_html_segment() {
    let id32 = "aBc123XYZ0aBc123XYZ0aBc123XYZ0aB";
    assert_eq!(
        file_id(&url(&format!(
            "https://rapidgator.net/file/{id32}/movie-name.html"
        ))),
        Some(id32)
    );
    assert_eq!(
        file_id(&url("https://rapidgator.net/file/123456/movie-name.html")),
        Some("123456")
    );
}

#[test]
fn file_id_rejects_unsupported_shapes() {
    // Not 32 chars and not purely numeric.
    assert_eq!(file_id(&url("https://rapidgator.net/file/short")), None);
    assert_eq!(file_id(&url("https://rapidgator.net/")), None);
    assert_eq!(
        file_id(&url("https://rapidgator.net/article/premium")),
        None
    );
    assert_eq!(file_id(&url("https://evil.example/file/123456")), None);
}

#[test]
fn parse_download_url_accepts_a_well_formed_url() {
    let parsed =
        parse_download_url("https://pr1.rapidgator.net/d/tok/release.rar").expect("valid URL");
    assert_eq!(
        parsed.as_str(),
        "https://pr1.rapidgator.net/d/tok/release.rar"
    );
}

#[test]
fn parse_download_url_rejects_a_malformed_url() {
    let failure = parse_download_url("not a url").expect_err("malformed URL");
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::INVALID_URL);
    assert!(
        failure.params.iter().any(|(name, _)| *name == "error"),
        "expected an `error` parameter"
    );
}

#[test]
fn error_from_envelope_treats_absent_and_200_status_as_success() {
    assert!(error_from_envelope(None, None, true).is_none());
    assert!(error_from_envelope(Some(200), None, true).is_none());
    assert!(error_from_envelope(Some(401), Some("Login or password is wrong"), true).is_some());
}

#[test]
fn classify_status_maps_traffic_and_storage_limit() {
    let by_status = classify_status(423, "Error: Exceeded traffic", true);
    assert!(matches!(by_status.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(by_status.code, messages::LIMIT_REACHED.0);

    // Reachable via message alone too, per JD's `||`.
    let by_message = classify_status(200, "You have Exceeded traffic for today", true);
    assert!(matches!(by_message.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(by_message.code, messages::LIMIT_REACHED.0);
}

#[test]
fn classify_status_maps_denied_by_ip_to_a_temporary_block_not_account_invalid() {
    let failure = classify_status(200, "Denied by IP", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(7200))));
    assert_eq!(failure.code, messages::IP_DENIED.0);
}

#[test]
fn classify_status_maps_login_throttling() {
    let failure = classify_status(200, "Please wait before trying again", true);
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(300))));
    assert_eq!(failure.code, messages::LOGIN_THROTTLED.0);
}

#[test]
fn classify_status_maps_premium_required_phrases() {
    for phrase in [
        "User is not PREMIUM",
        "This file can be downloaded by premium only",
        "You can download files up to 500MB",
    ] {
        let failure = classify_status(200, phrase, true);
        assert!(matches!(failure.kind, ErrorKind::AuthRequired), "{phrase}");
        assert_eq!(failure.code, messages::PREMIUM_REQUIRED.0, "{phrase}");
    }
}

#[test]
fn classify_status_maps_bad_credentials_phrases() {
    for phrase in [
        "Login or password is wrong",
        "Error: Error e-mail or password",
        "Password cannot be blank",
        "User is FROZEN",
        "Error: ACCOUNT LOCKED FOR VIOLATION OF OUR TERMS. PLEASE CONTACT SUPPORT.",
        "Parameter login or password is missing",
    ] {
        let failure = classify_status(200, phrase, true);
        assert!(
            matches!(failure.kind, ErrorKind::AccountInvalid),
            "{phrase}"
        );
        assert_eq!(failure.code, messages::BAD_CREDENTIALS.0, "{phrase}");
    }
    let failure = classify_status(401, "Wrong e-mail or password", true);
    assert!(matches!(failure.kind, ErrorKind::AccountInvalid));
    assert_eq!(failure.code, messages::BAD_CREDENTIALS.0);
}

#[test]
fn classify_status_maps_bare_401_to_session_invalid_when_unmatched() {
    let failure = classify_status(401, "Some unexpected body", true);
    assert!(matches!(failure.kind, ErrorKind::Transient(None)));
    assert_eq!(failure.code, messages::SESSION_INVALID.0);
}

#[test]
fn classify_status_maps_session_phrases() {
    for phrase in [
        "Error. Session doesn't exist",
        "This download session is not for you",
        "Session not found",
    ] {
        let failure = classify_status(200, phrase, true);
        assert!(
            matches!(failure.kind, ErrorKind::Transient(None)),
            "{phrase}"
        );
        assert_eq!(failure.code, messages::SESSION_INVALID.0, "{phrase}");
    }
}

#[test]
fn classify_status_maps_404_500_503() {
    let offline = classify_status(404, "Not found", true);
    assert!(matches!(offline.kind, ErrorKind::Offline));
    assert_eq!(offline.code, messages::FILE_OFFLINE.0);

    let server_500 = classify_status(500, "API error 500", true);
    assert!(matches!(server_500.kind, ErrorKind::Transient(Some(300))));
    assert_eq!(server_500.code, messages::SERVER_ERROR.0);

    let server_503 = classify_status(503, "Download temporarily unavailable", true);
    assert!(matches!(server_503.kind, ErrorKind::Transient(Some(1800))));
    assert_eq!(server_503.code, messages::SERVER_ERROR.0);
}

/// `file/download`'s 404 (`trust_404 = false`) must retry, not declare the file offline — JD's
/// documented API-bug workaround (`trustError404=false`); see this file's module doc.
#[test]
fn classify_status_maps_untrusted_404_to_a_transient_retry_not_offline() {
    let failure = classify_status(404, "Not found", false);
    assert!(matches!(failure.kind, ErrorKind::Transient(Some(60))));
    assert_eq!(failure.code, messages::DOWNLOAD_404_UNTRUSTED.0);

    // The bare-HTTP-status fallback makes the same distinction.
    let bare = ensure_http_status(404, false).expect_err("untrusted 404");
    assert!(matches!(bare.kind, ErrorKind::Transient(Some(60))));
    assert_eq!(bare.code, messages::DOWNLOAD_404_UNTRUSTED.0);
}

#[test]
fn classify_status_maps_ip_confirmation_required_to_a_temporary_retry() {
    let failure = classify_status(
        200,
        "Error: You requested login to your account from unusual Ip address",
        true,
    );
    // JD's exception class here is the same temporary `AccountUnavailableException` used for
    // "Denied by IP"/traffic-limit, not a permanent credential failure.
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(60))));
    assert_eq!(failure.code, messages::IP_CONFIRMATION_REQUIRED.0);
}

#[test]
fn classify_status_falls_back_to_a_transient_unknown_error() {
    let failure = classify_status(999, "Some new provider error", true);
    assert!(matches!(failure.kind, ErrorKind::Transient(Some(60))));
    assert_eq!(failure.code, messages::API_ERROR);
    assert!(failure.message.contains("Some new provider error"));
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| *name == "api_status" && value == "999")
    );
}

#[test]
fn ensure_http_status_maps_bare_codes() {
    assert!(ensure_http_status(200, true).is_ok());
    assert!(matches!(
        ensure_http_status(401, true).expect_err("401").kind,
        ErrorKind::AccountInvalid
    ));
    assert!(matches!(
        ensure_http_status(404, true).expect_err("404").kind,
        ErrorKind::Offline
    ));
    assert!(matches!(
        ensure_http_status(423, true).expect_err("423").kind,
        ErrorKind::RateLimited(Some(300))
    ));
    assert!(matches!(
        ensure_http_status(500, true).expect_err("500").kind,
        ErrorKind::Transient(Some(3600))
    ));
    assert!(matches!(
        ensure_http_status(503, true).expect_err("503").kind,
        ErrorKind::Transient(Some(300))
    ));
    let other = ensure_http_status(418, true).expect_err("418");
    assert!(matches!(other.kind, ErrorKind::Permanent));
    assert_eq!(other.code, messages::HTTP_ERROR);
}

#[test]
fn civil_date_formats_known_epoch_seconds() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(951_868_800), "2000-03-01");
    assert_eq!(civil_date(1_709_164_800), "2024-02-29"); // leap day
    assert_eq!(civil_date(1_798_761_600), "2027-01-01");
}

#[test]
fn premium_until_states_the_end_date_only_while_premium() {
    assert_eq!(
        premium_until(true, Some(1_798_761_600)).as_deref(),
        Some("2027-01-01")
    );
    assert_eq!(premium_until(false, None), None);
    // A `premium_end_time` on a non-premium account (shouldn't happen, but defensive) is
    // ignored, matching JD only reading it inside the `is_premium` branch.
    assert_eq!(premium_until(false, Some(1_798_761_600)), None);
}
