//! Unit tests for [`super`] (`crate::api::errors`), and the home of the full IMPL-VERIFY
//! provenance for this error classification (moved out of `api.rs`'s module doc, and later out
//! of `api/tests.rs`, to keep both files under the workspace's 500-line limit — see
//! plugin-common.md).
//!
//! IMPL-VERIFY: `K2SApi.handleErrorsAPI`'s complete per-branch enumeration (JD
//! `svn_trunk/src/jd/plugins/hoster/K2SApi.java`, revision 53214, lines 1350-1589), in the exact
//! order JD evaluates them, with this module's mapping decision for each:
//!
//! - **JSON parse failure** (lines 1352-1363): JD falls back to `checkResponseCodeErrors` (see
//!   the bare-HTTP-status arms below) then throws a generic "Invalid API response"
//!   (`AccountUnavailableException`/`PluginException(ERROR_TEMPORARILY_UNAVAILABLE)`, 3 minutes).
//!   Mirrored by [`super::ensure_http_status`], the fallback this crate's adapters use when the
//!   body doesn't parse as [`super::ErrorProbe`] at all; a status that isn't 400/401/403/404/410/
//!   451/429/5xx there falls through to `messages::INVALID_RESPONSE`-flavored handling at the
//!   call site rather than a dedicated `ErrorProbe`-less arm here (JD's own fallback has no
//!   distinct code either — it reuses `ERROR_TEMPORARILY_UNAVAILABLE`).
//! - **`errorCode` string enum** (lines 1369-1380): `"captcha_need_wait"` → IP temporarily banned
//!   for repeated captcha failures, 1-minute wait; `"captcha_need_wait_daily"` → permanently (for
//!   the day) banned, 30-minute wait. Both `AccountUnavailableException`. Any other string is
//!   logged and falls through to the numeric `code` field, exactly like a response that never had
//!   an `errorCode` at all. Mapped to [`messages::FLOOD`], `RateLimited{60}`/`RateLimited{1800}`.
//! - **No usable `errorCode`/`code` field at all** (lines 1385-1390): success — JD returns the
//!   parsed body as-is (after one last `checkResponseCodeErrors` bare-status sanity check).
//! - **`errorcode == 200 && status == "success"`** (lines 1396-1402): success, explicitly.
//! - **Sub-error unwrap for errorcode 21/22/42** (lines 1405-1415): the wrapper's own `code`/
//!   `message`/`timeRemaining` is replaced by `errors[0]`'s fields before the switch below runs,
//!   if `errors` is non-empty. Implemented as a pre-step in [`super::error_from_probe`].
//! - **`message` equals "File not available"** (lines 1423-1425, checked *before* the numeric
//!   switch, so it wins regardless of which errorcode accompanies it): `ERROR_FILE_NOT_FOUND` →
//!   [`messages::FILE_OFFLINE`], `Offline`.
//! - **1** — "Download count files exceed" (line 1427-1430): `ERROR_IP_BLOCKED`, JD's
//!   `ipBlockedOrAccountLimit` with `FREE_RECONNECTWAIT_MILLIS` (1 hour) since this plugin always
//!   has an account. → [`messages::DOWNLOAD_LIMIT_REACHED`], `RateLimited{3600}`.
//! - **2** — "Traffic limit exceed" (line 1431-1438): same 1-hour wait. →
//!   [`messages::TRAFFIC_EXHAUSTED`], `RateLimited{3600}` (the brief's explicitly named mapping).
//! - **3, 7** — file-size-exceeded / premium-only (line 1439-1444): `AccountRequiredException`. →
//!   [`messages::PREMIUM_REQUIRED`], `AuthRequired`.
//! - **4** — "You no can access to this file" (line 1445-1448): `ERROR_FATAL`, no retry. →
//!   [`messages::NO_ACCESS`], `Permanent`.
//! - **5** — "Please wait to download this file" (line 1449-1463): wait duration from the
//!   sub-error's `timeRemaining` (seconds, truncated at any decimal point), default 15 minutes. →
//!   [`messages::DOWNLOAD_WAIT`], `RateLimited{<dynamic>}`.
//! - **6** — free-account parallel-download cap (line 1464-1468): fixed 15-minute wait. →
//!   [`messages::TOO_MANY_PARALLEL`], `RateLimited{900}`.
//! - **9** — "store subscribers only" (line 1469-1474): `AccountRequiredException`. → same
//!   [`messages::PREMIUM_REQUIRED`] bucket as 3/7 (identical exception class and outcome).
//! - **8** — "This is private file" (line 1475-1479): `privateDownloadRestriction` →
//!   `ERROR_FATAL`, no retry. → [`messages::PRIVATE_FILE`], `Permanent`.
//! - **10** — "not authorized" / stale `auth_token` (line 1480-1484): dumps the cached token,
//!   `AccountUnavailableException`, 1 minute. This plugin never caches a token (fresh `/login`
//!   every call), so the "dump" is a no-op; only the retry semantics matter. →
//!   [`messages::SESSION_INVALID`], `Transient{60}`.
//! - **11, 42** (default sub-case, no `errors[0]` to unwrap into a more specific code) —
//!   "auth_token expired" / generic download-not-available (line 1485-1501):
//!   `AccountRequiredException`. → same [`messages::PREMIUM_REQUIRED`] bucket.
//! - **75** — "token not allowed from this IP" (line 1502-1509): same handling as 10. → same
//!   [`messages::SESSION_INVALID`] bucket.
//! - **20** — `ERROR_FILE_NOT_FOUND` (line 1510-1512). → [`messages::FILE_OFFLINE`], `Offline`.
//! - **21, 22** (reached only when the sub-error unwrap above found nothing to unwrap, or the
//!   unwrapped code isn't otherwise recognized) — file unavailable / blocked (line 1513-1522):
//!   `ERROR_TEMPORARILY_UNAVAILABLE`, no explicit wait. → [`messages::TEMPORARILY_UNAVAILABLE`],
//!   `Transient{None}`.
//! - **23** — "file_id is folder" (line 1523-1525): `ERROR_FILE_NOT_FOUND`. → same
//!   [`messages::FILE_OFFLINE`] bucket as 20.
//! - **30, 33** — image captcha / reCaptcha demanded (line 1526-1538): on the raw `/login`
//!   response specifically, JD intercepts this even earlier (before `handleErrorsAPI` runs at
//!   all, matched via a regex on the raw body) and drives an interactive captcha-solve retry loop
//!   capped at one attempt; reached here as a fallback for any other endpoint, where JD throws
//!   `ERROR_PLUGIN_DEFECT` since a captcha demand mid-download is unexpected in premium mode. This
//!   plugin has no captcha-solving capability on any endpoint, so both cases collapse to one
//!   unconditional, non-retryable outcome. → [`messages::LOGIN_CAPTCHA`], `NeedsCaptcha`.
//! - **31** — captcha answer rejected (line 1539-1541): `ERROR_CAPTCHA`. Reachable only from the
//!   account-less free flow, the one path that submits a captcha answer (`api::free`), so it gets
//!   its own [`messages::CAPTCHA_REJECTED`]/`CaptchaFailed` rather than sharing 30/33's "a captcha
//!   is required" bucket.
//! - **40** — wrong free-download key (line 1542-1544): `ERROR_PLUGIN_DEFECT`, no wait — the same
//!   non-retryable outcome as the default arm below. Unreachable in practice (this plugin never
//!   sends `free_download_key`); no dedicated code, folds into the generic default arm below
//!   rather than getting its own `messages` constant, since both land on the identical
//!   `Permanent`/`api_error` outcome anyway.
//! - **41, 70, 72** — bad credentials / legacy alias / account banned (line 1545-1551): dumps the
//!   token, `AccountInvalidException`. → [`messages::BAD_CREDENTIALS`], `AccountInvalid`.
//! - **71** — "Login attempt was exceed" (line 1552-1560): `AccountUnavailableException`, 31
//!   minutes. → [`messages::FLOOD`], `RateLimited{1860}`.
//! - **73** — network/IP not allowed to reach `k2s.cc` (line 1561-1567): `AccountUnavailableException`
//!   when an account is present (always true here), 6 hours. → [`messages::NETWORK_RESTRICTED`],
//!   `RateLimited{21600}`.
//! - **74** — unknown login error (line 1568-1574): `AccountInvalidException("Account has been
//!   banned")` when an account is present. → same [`messages::BAD_CREDENTIALS`] bucket as 41/70/72
//!   (same exception class; JD's fixed message string differs from the 41/70/72 arms' dynamic
//!   server message, not replicated since this module's messages carry the live server text via
//!   the `message` parameter, not a hardcoded string per sub-case).
//! - **76** — "Account stolen" (line 1575-1577): `AccountInvalidException`. → same
//!   [`messages::BAD_CREDENTIALS`] bucket.
//! - **default** — any other errorcode (line 1578-1582): one last `checkResponseCodeErrors` bare-
//!   status check, then `ERROR_PLUGIN_DEFECT` with **no wait time** — non-retryable in JD. →
//!   [`messages::API_ERROR`], `Permanent` (the task brief independently specifies "unknown ->
//!   Permanent keep2share.api_error" — JD and the brief agree here. An earlier revision of this
//!   module mapped this arm `Transient{300}`, a deviation from *both* JD and the brief with no
//!   supporting evidence of its own; corrected after review. Unlike the Rapidgator/LinkSnappy
//!   plugins in this workspace, whose own JD references make their unknown-code fallback
//!   explicitly retryable, K2S's JD reference does not, so this plugin does not borrow their
//!   convention here).
//! - **Bare HTTP status** (JD's `checkResponseCodeErrors`, lines 1591-1599, invoked whenever no
//!   JSON `errorCode`/`code` was found or the body didn't parse at all): `400` → 5-minute retry
//!   ("This may happen after any request even if the request itself is done right"); `429` →
//!   JD's own delay here is 3 minutes, but plugin-common's stated `429 → RateLimited` convention
//!   is followed instead (`RateLimited{None}`) for consistency with every other plugin in this
//!   workspace, since JD's browser layer (`createNewBrowserInstance`/`prepAPI`) allows 400/401/
//!   403/406/429/503/520/522 through to body-level handling rather than throwing on them
//!   directly — plugin-common's 401/403 → `AccountInvalid`, 404/410/451 → `Offline`, 5xx →
//!   `Transient` conventions are applied for any bare status JD's own plugin never documents a
//!   distinct handler for. See [`super::ensure_http_status`].
//! - **`/login`'s own captcha short-circuit** (lines 1170-1240, outside `handleErrorsAPI`): JD
//!   detects `"errorCode":30` (image captcha) or `"errorCode":33` (reCaptcha) directly in the raw
//!   `/login` response body and only *then* calls `handleErrorsAPI` if no captcha was demanded.
//!   Functionally identical to this module's handling: `/login`'s `errorCode` still flows through
//!   [`super::error_from_probe`] → [`super::classify_errorcode`]'s 30/33 arm the same way any other
//!   endpoint's would, so no special-casing by endpoint is needed here.

use super::*;

fn probe(json: &str) -> ErrorProbe {
    serde_json::from_str(json).expect("valid ErrorProbe JSON")
}

#[test]
fn error_from_probe_treats_absent_and_success_as_ok() {
    assert!(error_from_probe(&probe("{}")).is_none());
    assert!(error_from_probe(&probe(r#"{"status":"success","code":200}"#)).is_none());
    assert!(
        error_from_probe(&probe(
            r#"{"available_traffic":123456,"account_expires":false}"#
        ))
        .is_none()
    );
}

#[test]
fn error_from_probe_string_errorcode_maps_captcha_flood_variants() {
    let short = error_from_probe(&probe(
        r#"{"status":"error","code":400,"message":"captcha need wait","errorCode":"captcha_need_wait"}"#,
    ))
    .expect("captcha_need_wait");
    assert!(matches!(short.kind, ErrorKind::RateLimited(Some(60))));
    assert_eq!(short.code, messages::FLOOD.0);

    let daily = error_from_probe(&probe(
        r#"{"status":"error","code":400,"message":"captcha need wait daily","errorCode":"captcha_need_wait_daily"}"#,
    ))
    .expect("captcha_need_wait_daily");
    assert!(matches!(daily.kind, ErrorKind::RateLimited(Some(1800))));
    assert_eq!(daily.code, messages::FLOOD.0);
}

#[test]
fn error_from_probe_prefers_numeric_errorcode_over_top_level_code() {
    // errorCode (number) wins over `code` when both are present.
    let failure = error_from_probe(&probe(r#"{"status":"error","code":406,"errorCode":20}"#))
        .expect("file not found");
    assert!(matches!(failure.kind, ErrorKind::Offline));
    assert_eq!(failure.code, messages::FILE_OFFLINE.0);
}

#[test]
fn error_from_probe_falls_back_to_code_when_errorcode_is_absent_or_a_string() {
    // No `errorCode` at all -> falls back to `code`.
    let failure = error_from_probe(&probe(r#"{"status":"error","code":20}"#)).expect("code 20");
    assert!(matches!(failure.kind, ErrorKind::Offline));

    // Unknown string `errorCode` -> also falls back to `code`.
    let failure = error_from_probe(&probe(
        r#"{"status":"error","code":20,"errorCode":"some_future_enum"}"#,
    ))
    .expect("code 20 via string fallback");
    assert!(matches!(failure.kind, ErrorKind::Offline));
}

#[test]
fn error_from_probe_message_file_not_available_wins_regardless_of_errorcode() {
    let failure = error_from_probe(&probe(
        r#"{"status":"error","code":406,"errorCode":42,"message":"File not available"}"#,
    ))
    .expect("file not available");
    assert!(matches!(failure.kind, ErrorKind::Offline));
    assert_eq!(failure.code, messages::FILE_OFFLINE.0);
}

#[test]
fn error_from_probe_unwraps_generic_wrapper_codes_via_sub_errors() {
    // errorcode 42 wrapping a traffic-limit (2) sub-error.
    let failure = error_from_probe(&probe(
        r#"{"status":"error","code":406,"errorCode":42,"message":"Download not available","errors":[{"code":2,"message":"Traffic limit exceed"}]}"#,
    ))
    .expect("unwrapped traffic limit");
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(3600))));
    assert_eq!(failure.code, messages::TRAFFIC_EXHAUSTED.0);

    // errorcode 21 wrapping a download-wait (5) sub-error with a timeRemaining.
    let failure = error_from_probe(&probe(
        r#"{"status":"error","code":406,"errorCode":21,"errors":[{"code":5,"timeRemaining":"2521.000000"}]}"#,
    ))
    .expect("unwrapped download wait");
    assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(2521))));
    assert_eq!(failure.code, messages::DOWNLOAD_WAIT.0);

    // 42 with no sub-errors falls back to its own default handling (premium required).
    let failure = error_from_probe(&probe(r#"{"status":"error","code":406,"errorCode":42}"#))
        .expect("bare 42");
    assert!(matches!(failure.kind, ErrorKind::AuthRequired));
    assert_eq!(failure.code, messages::PREMIUM_REQUIRED.0);
}

#[test]
fn classify_errorcode_maps_limit_and_wait_codes() {
    let count = classify_errorcode(1, "Download count files exceed", None);
    assert!(matches!(count.kind, ErrorKind::RateLimited(Some(3600))));
    assert_eq!(count.code, messages::DOWNLOAD_LIMIT_REACHED.0);

    let traffic = classify_errorcode(2, "Traffic limit exceed", None);
    assert!(matches!(traffic.kind, ErrorKind::RateLimited(Some(3600))));
    assert_eq!(traffic.code, messages::TRAFFIC_EXHAUSTED.0);

    let wait_default = classify_errorcode(5, "Please wait", None);
    assert!(matches!(
        wait_default.kind,
        ErrorKind::RateLimited(Some(900))
    ));
    assert_eq!(wait_default.code, messages::DOWNLOAD_WAIT.0);

    let too_many_parallel = classify_errorcode(6, "Too many parallel", None);
    assert!(matches!(
        too_many_parallel.kind,
        ErrorKind::RateLimited(Some(900))
    ));
    assert_eq!(too_many_parallel.code, messages::TOO_MANY_PARALLEL.0);
}

#[test]
fn classify_errorcode_maps_premium_required_codes() {
    for code in [3, 7, 9, 11, 42] {
        let failure = classify_errorcode(code, "Premium only", None);
        assert!(
            matches!(failure.kind, ErrorKind::AuthRequired),
            "code {code}"
        );
        assert_eq!(failure.code, messages::PREMIUM_REQUIRED.0, "code {code}");
    }
}

#[test]
fn classify_errorcode_maps_permanent_access_codes() {
    let no_access = classify_errorcode(4, "No access", None);
    assert!(matches!(no_access.kind, ErrorKind::Permanent));
    assert_eq!(no_access.code, messages::NO_ACCESS.0);

    let private = classify_errorcode(8, "This is private file", None);
    assert!(matches!(private.kind, ErrorKind::Permanent));
    assert_eq!(private.code, messages::PRIVATE_FILE.0);
}

#[test]
fn classify_errorcode_maps_session_invalid_codes() {
    for code in [10, 75] {
        let failure = classify_errorcode(code, "Not authorized", None);
        assert!(
            matches!(failure.kind, ErrorKind::Transient(Some(60))),
            "code {code}"
        );
        assert_eq!(failure.code, messages::SESSION_INVALID.0, "code {code}");
    }
}

#[test]
fn classify_errorcode_maps_offline_codes() {
    for code in [20, 23] {
        let failure = classify_errorcode(code, "Not found", None);
        assert!(matches!(failure.kind, ErrorKind::Offline), "code {code}");
        assert_eq!(failure.code, messages::FILE_OFFLINE.0, "code {code}");
    }
}

#[test]
fn classify_errorcode_maps_temporarily_unavailable() {
    for code in [21, 22] {
        let failure = classify_errorcode(code, "Unavailable", None);
        assert!(
            matches!(failure.kind, ErrorKind::Transient(None)),
            "code {code}"
        );
        assert_eq!(
            failure.code,
            messages::TEMPORARILY_UNAVAILABLE.0,
            "code {code}"
        );
    }
}

#[test]
fn classify_errorcode_maps_captcha_family_to_needs_captcha() {
    for code in [30, 33] {
        let failure = classify_errorcode(code, "Captcha required", None);
        assert!(
            matches!(failure.kind, ErrorKind::NeedsCaptcha),
            "code {code}"
        );
        assert_eq!(failure.code, messages::LOGIN_CAPTCHA.0, "code {code}");
    }
}

/// Errorcode 31 is a *rejected* answer, not a captcha demand — see this module's doc.
#[test]
fn classify_errorcode_maps_a_rejected_captcha_answer_to_captcha_failed() {
    let failure = classify_errorcode(31, "Wrong captcha", None);
    assert!(matches!(failure.kind, ErrorKind::CaptchaFailed));
    assert_eq!(failure.code, messages::CAPTCHA_REJECTED.0);
}

#[test]
fn classify_errorcode_maps_bad_credentials_codes() {
    for code in [41, 70, 72, 74, 76] {
        let failure = classify_errorcode(code, "Invalid username/password", None);
        assert!(
            matches!(failure.kind, ErrorKind::AccountInvalid),
            "code {code}"
        );
        assert_eq!(failure.code, messages::BAD_CREDENTIALS.0, "code {code}");
    }
}

#[test]
fn classify_errorcode_maps_flood_and_network_restricted() {
    let flood = classify_errorcode(71, "Login attempt was exceed", None);
    assert!(matches!(flood.kind, ErrorKind::RateLimited(Some(1860))));
    assert_eq!(flood.code, messages::FLOOD.0);

    let network = classify_errorcode(73, "You can not access k2s.cc", None);
    assert!(matches!(network.kind, ErrorKind::RateLimited(Some(21_600))));
    assert_eq!(network.code, messages::NETWORK_RESTRICTED.0);
}

#[test]
fn classify_errorcode_falls_back_to_a_permanent_unknown_error() {
    // Includes errorcode 40 (unreachable in this plugin's premium-only flow, per its doc comment).
    // JD's own default arm (`ERROR_PLUGIN_DEFECT`, no wait) and the task brief ("unknown ->
    // Permanent keep2share.api_error") agree this is non-retryable.
    for code in [40, 999] {
        let failure = classify_errorcode(code, "Some new provider error", None);
        assert!(matches!(failure.kind, ErrorKind::Permanent), "code {code}");
        assert_eq!(failure.code, messages::API_ERROR, "code {code}");
        assert!(failure.message.contains("Some new provider error"));
        assert!(
            failure
                .params
                .iter()
                .any(|(name, value)| *name == "api_status" && value == &code.to_string()),
            "code {code}"
        );
    }
}

#[test]
fn ensure_http_status_maps_bare_codes() {
    assert!(ensure_http_status(200).is_ok());
    assert!(matches!(
        ensure_http_status(400).expect_err("400").kind,
        ErrorKind::Transient(Some(300))
    ));
    assert!(matches!(
        ensure_http_status(401).expect_err("401").kind,
        ErrorKind::AccountInvalid
    ));
    assert!(matches!(
        ensure_http_status(403).expect_err("403").kind,
        ErrorKind::AccountInvalid
    ));
    assert!(matches!(
        ensure_http_status(404).expect_err("404").kind,
        ErrorKind::Offline
    ));
    assert!(matches!(
        ensure_http_status(429).expect_err("429").kind,
        ErrorKind::RateLimited(None)
    ));
    assert!(matches!(
        ensure_http_status(503).expect_err("503").kind,
        ErrorKind::Transient(None)
    ));
    let other = ensure_http_status(418).expect_err("418");
    assert!(matches!(other.kind, ErrorKind::Permanent));
    assert_eq!(other.code, messages::HTTP_ERROR);
}
