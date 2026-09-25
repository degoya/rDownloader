//! Target-independent AllDebrid API v4.1 logic: request bodies, response envelopes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! IMPL-VERIFY (against JD's `AllDebridCom.java`, the living reference — see task-5-report.md
//! for the full list): `/link/unlock` and `/link/delayed` are `POST` with an
//! `application/x-www-form-urlencoded` body (`link=`/`id=`), not `GET` with a query string as
//! the brief first assumed; no `agent` query parameter is sent (JD never sends one); there is no
//! `/link/infos` batch-check endpoint, so `check()` is left unimplemented (host trait default);
//! `/user/hosts` nests each hoster's domains under `data.hosts.<key>.domains` and
//! `data.streams.<key>.domains`, not a flat array.

use std::collections::BTreeMap;

use serde::Deserialize;
use url::form_urlencoded;

use crate::messages;

/// `rd-provider-registry`'s `alldebrid` row: `secret_reference`.
pub(crate) const API_KEY_REFERENCE: &str = "alldebrid_api_key";

pub(crate) const API_BASE: &str = "https://api.alldebrid.com/v4.1";

/// AllDebrid is a multihoster: it claims any http(s) URL (mirrors `premiumize::matches`).
pub(crate) fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// `application/x-www-form-urlencoded` body for `POST /link/unlock`.
pub(crate) fn unlock_body(link: &str) -> Vec<u8> {
    form_body(&[("link", link)])
}

/// `application/x-www-form-urlencoded` body for `POST /link/delayed`.
pub(crate) fn delayed_body(id: &str) -> Vec<u8> {
    form_body(&[("id", id)])
}

fn form_body(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish().into_bytes()
}

/// Validates the raw download URL string `link/unlock`'s `data.link` carries. Shared so a
/// malformed URL from the API produces the exact same `alldebrid.invalid_url` failure on both
/// the native and guest adapters instead of one erroring and the other silently forwarding a
/// URL that later fails to parse deeper in the pipeline.
pub(crate) fn parse_download_url(raw: &str) -> Result<url::Url, ApiFailure> {
    url::Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// Generic `{"status": "success"|"error", "data": {...}, "error": {...}}` envelope every
/// AllDebrid v4.1 endpoint answers with.
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) status: String,
    #[serde(default = "Option::default")]
    pub(crate) data: Option<T>,
    pub(crate) error: Option<ApiError>,
}

#[derive(Deserialize)]
pub(crate) struct ApiError {
    pub(crate) code: String,
    pub(crate) message: String,
}

/// `data` of `POST /link/unlock`.
#[derive(Deserialize)]
pub(crate) struct UnlockData {
    pub(crate) link: Option<String>,
    pub(crate) filename: Option<String>,
    pub(crate) filesize: Option<u64>,
    /// Present (a numeric or string id) when AllDebrid must fetch the file server-side before
    /// it can be downloaded. See <https://docs.alldebrid.com/#delayed-links>.
    #[serde(default)]
    pub(crate) delayed: Option<serde_json::Value>,
}

impl UnlockData {
    /// The `delayed` id rendered as a string for `POST /link/delayed`, if present and non-empty.
    pub(crate) fn delayed_id(&self) -> Option<String> {
        match self.delayed.as_ref()? {
            serde_json::Value::String(value) if !value.is_empty() => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        }
    }
}

/// `data` of `POST /link/delayed`: `status` is `1` = still processing, `2` = available,
/// `3` = error (JD `AllDebridCom#cacheDLChecker`).
#[derive(Deserialize)]
pub(crate) struct DelayedData {
    pub(crate) status: Option<u32>,
}

/// `data` of `GET /user`.
#[derive(Deserialize)]
pub(crate) struct UserData {
    pub(crate) user: UserInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserInfo {
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) is_premium: bool,
}

/// `data` of `GET /user/hosts`: hoster domains grouped by capability (`hosts`, `streams`), each
/// value keyed by an opaque hoster id.
#[derive(Default, Deserialize)]
pub(crate) struct HostsData {
    #[serde(default)]
    pub(crate) hosts: BTreeMap<String, HostEntry>,
    #[serde(default)]
    pub(crate) streams: BTreeMap<String, HostEntry>,
}

#[derive(Deserialize)]
pub(crate) struct HostEntry {
    #[serde(default)]
    pub(crate) domains: Vec<String>,
}

/// Lower-cases, deduplicates and sorts every hoster domain from both capability groups (mirrors
/// `premiumize::services::merge_hosters`).
pub(crate) fn merge_hosters(data: HostsData) -> Vec<String> {
    let mut hosters: Vec<String> = data
        .hosts
        .into_values()
        .chain(data.streams.into_values())
        .flat_map(|entry| entry.domains)
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    hosters.sort();
    hosters.dedup();
    hosters
}

/// Failure classification independent of the native (`rd_core::Failure`) and WASM
/// (WIT-generated `Failure`) representations; both adapters convert this into their own type.
#[derive(Debug)]
pub(crate) struct ApiFailure {
    pub(crate) kind: ErrorKind,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) params: Vec<(&'static str, String)>,
}

/// Mirrors `rd_core::FailureKind` / the WIT `failure-kind` variant, without depending on either.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    #[allow(dead_code)] // AllDebrid's JSON API never challenges with a captcha.
    NeedsCaptcha,
    Unsupported,
}

fn coded(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// Like [`coded`], but attaches the provider's raw error code as an `api_code` param (same key
/// the catch-all branch of [`classify_error`] uses) so the UI can show which specific provider
/// error occurred, even though the stable `code`/`message` stay generic across the whole
/// permanent-auth-error group.
fn coded_with_provider_code(
    kind: ErrorKind,
    (code, message): (&'static str, &str),
    provider_code: &str,
) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: vec![("api_code", provider_code.to_owned())],
    }
}

/// Classifies a `{"status":"error","error":{"code":...,"message":...}}` envelope by the
/// provider's `error.code`. Every branch mirrors JDownloader's `AllDebridCom#handleErrors`
/// (`https://docs.alldebrid.com/#all-errors`):
/// - `AUTH_MISSING_APIKEY`/`AUTH_BAD_APIKEY`/`AUTH_USER_BANNED`/`ACCOUNT_INVALID` are JD's
///   *permanent* account errors (bad/banned key).
/// - `AUTH_BLOCKED` is one of JD's *temporary* account errors (5-minute retry, alongside
///   `MAINTENANCE`) — **not** grouped with the permanent auth failures above. The task brief
///   listed it next to `AUTH_BAD_APIKEY`/`AUTH_USER_BANNED` as `AccountInvalid`; JD's source
///   contradicts that (`accountErrorsTemporary`, not `accountErrorsPermanent`), so this follows
///   JD and reports it as transient instead — see task-5-report.md.
/// - `LINK_HOST_NOT_SUPPORTED`/`LINK_HOST_UNAVAILABLE`/`LINK_HOST_FULL`/
///   `LINK_HOST_LIMIT_REACHED`/`USER_LINK_INVALID` are JD's `downloadErrorsHostUnavailable`
///   bucket (identical handling for all five); the brief named only the first two, the other
///   three are added here for parity with JD's grouping.
fn classify_error(code: &str, message: &str) -> ApiFailure {
    match code {
        "AUTH_MISSING_APIKEY" | "AUTH_BAD_APIKEY" | "AUTH_USER_BANNED" | "ACCOUNT_INVALID" => {
            coded_with_provider_code(ErrorKind::AccountInvalid, messages::AUTH_INVALID, code)
        }
        "LINK_DOWN" | "LINK_NOT_FOUND" | "LINK_ERROR" => {
            coded(ErrorKind::Offline, messages::LINK_DOWN)
        }
        "LINK_HOST_NOT_SUPPORTED"
        | "LINK_HOST_UNAVAILABLE"
        | "LINK_HOST_FULL"
        | "LINK_HOST_LIMIT_REACHED"
        | "USER_LINK_INVALID" => coded(ErrorKind::Unsupported, messages::HOST_UNSUPPORTED),
        "MUST_BE_PREMIUM" | "FREE_TRIAL_LIMIT_REACHED" => {
            coded(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED)
        }
        "LINK_TEMPORARY_UNAVAILABLE" | "MAINTENANCE" | "AUTH_BLOCKED" => coded(
            ErrorKind::Transient(Some(300)),
            messages::TEMPORARILY_UNAVAILABLE,
        ),
        "LINK_PASS_PROTECTED" => coded(ErrorKind::Permanent, messages::PASSWORD_PROTECTED),
        _ => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(code, message),
            params: vec![
                ("api_code", code.to_owned()),
                ("message", message.to_owned()),
            ],
        },
    }
}

/// Checks an envelope's `status`/`error` pair; `None` for a `"success"` envelope.
pub(crate) fn error_from_status(status: &str, error: Option<&ApiError>) -> Option<ApiFailure> {
    if !status.eq_ignore_ascii_case("error") {
        return None;
    }
    Some(match error {
        Some(error) => classify_error(&error.code, &error.message),
        None => classify_error("UNKNOWN", "Unknown AllDebrid API error"),
    })
}

/// Maps an HTTP status the JSON envelope doesn't otherwise explain.
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(coded(ErrorKind::AccountInvalid, messages::AUTH_INVALID)),
        404 | 410 | 451 => Err(coded(ErrorKind::Offline, messages::LINK_DOWN)),
        429 => Err(coded(ErrorKind::RateLimited(None), messages::RATE_LIMITED)),
        500..=599 => Err(coded(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_accepts_only_http_and_https() {
        assert!(matches("http"));
        assert!(matches("https"));
        assert!(!matches("ftp"));
        assert!(!matches("magnet"));
    }

    #[test]
    fn unlock_body_encodes_the_link_form_field() {
        assert_eq!(
            unlock_body("https://example.test/f/abc"),
            b"link=https%3A%2F%2Fexample.test%2Ff%2Fabc"
        );
    }

    #[test]
    fn delayed_body_encodes_the_id_form_field() {
        assert_eq!(delayed_body("42"), b"id=42");
    }

    #[test]
    fn delayed_id_reads_numeric_and_string_ids() {
        let numeric = UnlockData {
            link: None,
            filename: None,
            filesize: None,
            delayed: Some(serde_json::json!(42)),
        };
        assert_eq!(numeric.delayed_id().as_deref(), Some("42"));

        let text = UnlockData {
            link: None,
            filename: None,
            filesize: None,
            delayed: Some(serde_json::json!("abc-123")),
        };
        assert_eq!(text.delayed_id().as_deref(), Some("abc-123"));

        let absent = UnlockData {
            link: None,
            filename: None,
            filesize: None,
            delayed: None,
        };
        assert_eq!(absent.delayed_id(), None);
    }

    #[test]
    fn parse_download_url_accepts_a_well_formed_url() {
        let parsed =
            parse_download_url("https://cdn.alldebrid.com/dl/tok/release.rar").expect("valid URL");
        assert_eq!(
            parsed.as_str(),
            "https://cdn.alldebrid.com/dl/tok/release.rar"
        );
    }

    #[test]
    fn parse_download_url_rejects_a_malformed_url() {
        let failure = parse_download_url("not a url").expect_err("malformed URL");
        assert!(matches!(failure.kind, ErrorKind::Permanent));
        assert_eq!(failure.code, messages::INVALID_URL);
        assert!(failure.params.iter().any(|(name, _)| *name == "error"));
    }

    #[test]
    fn error_from_status_requires_an_explicit_error_status() {
        assert!(error_from_status("success", None).is_none());
        assert!(error_from_status("ERROR", None).is_some());
    }

    #[test]
    fn classify_error_maps_permanent_account_errors() {
        for provider_code in [
            "AUTH_MISSING_APIKEY",
            "AUTH_BAD_APIKEY",
            "AUTH_USER_BANNED",
            "ACCOUNT_INVALID",
        ] {
            let failure = classify_error(provider_code, "The auth apikey is invalid");
            assert!(
                matches!(failure.kind, ErrorKind::AccountInvalid),
                "{provider_code}"
            );
            assert_eq!(failure.code, messages::AUTH_INVALID.0, "{provider_code}");
            // Finding 2 (review fix): the provider's own error code must be attached so the UI
            // can tell AUTH_BAD_APIKEY apart from AUTH_USER_BANNED etc., even though they all
            // share the same stable `alldebrid.auth_invalid` code/message.
            assert!(
                failure
                    .params
                    .iter()
                    .any(|(name, value)| *name == "api_code" && value == provider_code),
                "{provider_code}: missing api_code param"
            );
        }
    }

    #[test]
    fn classify_error_treats_auth_blocked_as_temporary_not_account_invalid() {
        // Deviation from the brief: JD's `accountErrorsTemporary` set includes AUTH_BLOCKED
        // (5-minute retry, same bucket as MAINTENANCE), not `accountErrorsPermanent`.
        let failure = classify_error("AUTH_BLOCKED", "Temporarily blocked");
        assert!(matches!(failure.kind, ErrorKind::Transient(Some(300))));
        assert_eq!(failure.code, messages::TEMPORARILY_UNAVAILABLE.0);
    }

    #[test]
    fn classify_error_maps_host_unavailable_group() {
        for code in [
            "LINK_HOST_NOT_SUPPORTED",
            "LINK_HOST_UNAVAILABLE",
            "LINK_HOST_FULL",
            "LINK_HOST_LIMIT_REACHED",
            "USER_LINK_INVALID",
        ] {
            let failure = classify_error(code, "host issue");
            assert!(matches!(failure.kind, ErrorKind::Unsupported), "{code}");
            assert_eq!(failure.code, messages::HOST_UNSUPPORTED.0, "{code}");
        }
    }

    #[test]
    fn classify_error_maps_unknown_codes_to_the_generic_bucket() {
        let failure = classify_error("SOME_NEW_CODE", "brand new failure");
        assert!(matches!(failure.kind, ErrorKind::Permanent));
        assert_eq!(failure.code, messages::API_ERROR);
        assert!(failure.message.contains("brand new failure"));
        assert!(
            failure
                .params
                .iter()
                .any(|(name, value)| *name == "api_code" && value == "SOME_NEW_CODE")
        );
    }

    #[test]
    fn merge_hosters_flattens_dedupes_and_sorts() {
        let data: HostsData = serde_json::from_str(
            r#"{
                "hosts": {
                    "1fichier": {"name": "1fichier", "domains": ["1fichier.com", "1FICHIER.COM"]},
                    "rapidgator": {"name": "rapidgator", "domains": ["rapidgator.net"]}
                },
                "streams": {
                    "youtube": {"name": "youtube", "domains": ["youtube.com", " "]}
                }
            }"#,
        )
        .expect("parse");
        assert_eq!(
            merge_hosters(data),
            vec!["1fichier.com", "rapidgator.net", "youtube.com"]
        );
    }
}
