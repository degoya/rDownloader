//! Target-independent Debrid-Link API v2 logic: request bodies, response envelopes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! IMPL-VERIFY (against JD's `DebridLinkCom.java`, the living reference; the task brief's URL is
//! correct at `svn_trunk/src/jd/plugins/hoster/DebridLinkCom.java`):
//!
//! - **Auth flavor**: JD authenticates with OAuth2 (device-code flow, `POST /api/oauth/token`,
//!   `Authorization: Bearer <access_token>`); it never uses a personal API key. Debrid-Link does
//!   offer personal API keys (`debrid-link.com/webapp/apikey`, referenced by the task brief), and
//!   JD's own error message for `badToken` reads "For token/key auth, you must create a new one"
//!   — confirming the *same* `Authorization: Bearer <token>` header accepts a personal API key,
//!   the API just does not distinguish the two token flavors at the header level. This plugin
//!   sends `Authorization: Bearer {{secret:debridlink_api_key}}` per the brief; unverified beyond
//!   that inference — flag for live testing.
//! - **Resolve endpoint**: `POST /api/v2/downloader/add` with an `application/x-www-form-urlencoded`
//!   body (`url=`), matching the brief. JD only reads `value.chunk` and `value.downloadUrl` from
//!   the response — it never reads a `name`/`size` field, so whether the API actually returns
//!   them is unverified from JD alone; both are modeled here as optional and simply left `None`
//!   if absent, same risk profile as leaving them out.
//! - **Account endpoint**: `GET /api/v2/account/infos` returns `value.accountType` (`0` = free,
//!   `1` = premium, `2` = lifetime), `value.premiumLeft` (seconds, `accountType == 1` only) and
//!   `value.email` (JD sets it as the account's username label). JD never reads a `pseudo` field;
//!   it is modeled here as optional per the brief and preferred over `email` when present, same
//!   fallback risk as `name`/`size` above.
//! - **Hosters endpoint**: `GET /api/v2/downloader/hosts?keys=status,isFree,name,domains` (JD's
//!   `fetchAccountInfo`), **not** `/downloader/domains` as the brief first guessed. `value` is a
//!   JSON *array* of `{name, status, isFree, domains}` objects, not a domain-keyed map.
//! - **Error envelope**: `{"success": bool, "error": "<key>", "value": ...}` — `error` is a bare
//!   string key with no accompanying provider message (unlike AllDebrid's `{code, message}`
//!   pair); friendly text comes from JD's own static `errorKeyToMessageMap`, not the response.
//! - **Error classification deviates from the brief in two places**, both because JD's actual
//!   `errHandling` buckets contradict it (see `classify_error` for the full mapping and
//!   per-bucket JD line references):
//!   1. `notDebrid`/`hostNotValid`/`notFreeHost` are brief-classified `Unsupported`, but JD's
//!      `downloadErrorsHostUnavailable` set treats all nine of its members (these three plus
//!      `maintenanceHost`/`noServerHost`/`disabledServerHost`/`freeServerOverload`, which the
//!      brief omitted) identically: a 5-minute link-level retry (`mhm.putError`), not a permanent
//!      capability gap. `debridlink.host_unsupported` keeps the brief's message/code (still useful
//!      to tell "this host" apart from "the server"), but is mapped `Transient{300}`.
//!   2. `floodDetected`'s own JD message text reads "API rate limit reached for the endpoint,
//!      retry after 1 hour", contradicting the brief's `RateLimited{600}` — even though JD's
//!      *code* enforces a flat 5-minute account pause for the whole `accountErrorsTemporary`
//!      bucket (a JD-internal scheduling detail, not the provider's documented reset window).
//!      This plugin follows the documented 1-hour reset: `RateLimited{3600}`.
//!
//!   `noLink`/`authorization`, named in the brief, do not appear in JD's `errorKeyToMessageMap` at
//!   all and are dropped; they fall through to the generic `debridlink.api_error` bucket like any
//!   other unrecognized key. `accountLocked`/`maintenanceHost`/`noServerHost`/`disabledServerHost`
//!   are JD-verified siblings of codes the brief did name and are added to the matching buckets
//!   for parity with JD's own grouping. `badFileUrl`/`badFilePassword`/`fileNotAvailable`
//!   (JD's `downloadErrorsFileUnavailable`) and `hidedToken` are outside the brief's scope and are
//!   intentionally left in the generic fallback bucket rather than growing the code surface
//!   further; they still surface with their raw key via `api_code`.

use serde::Deserialize;
use url::form_urlencoded;

use crate::messages;

/// `rd-provider-registry`'s `debridlink` row: `secret_reference`.
pub(crate) const API_KEY_REFERENCE: &str = "debridlink_api_key";

pub(crate) const API_BASE: &str = "https://debrid-link.com/api/v2";

/// Debrid-Link is a multihoster: it claims any http(s) URL (mirrors `premiumize`/`alldebrid`).
pub(crate) fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// `application/x-www-form-urlencoded` body for `POST /downloader/add`.
pub(crate) fn add_body(link: &str) -> Vec<u8> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("url", link);
    serializer.finish().into_bytes()
}

/// Validates the raw download URL string `downloader/add`'s `value.downloadUrl` carries. Shared
/// so a malformed URL from the API produces the exact same `debridlink.invalid_url` failure on
/// both the native and guest adapters instead of one erroring and the other silently forwarding a
/// URL that later fails to parse deeper in the pipeline.
pub(crate) fn parse_download_url(raw: &str) -> Result<url::Url, ApiFailure> {
    url::Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// Generic `{"success": bool, "error": "<key>"|null, "value": {...}|null}` envelope every
/// Debrid-Link v2 endpoint answers with.
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) success: bool,
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default = "Option::default")]
    pub(crate) value: Option<T>,
}

/// `value` of `POST /downloader/add`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddData {
    pub(crate) download_url: Option<String>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) size: Option<u64>,
}

/// `value` of `GET /account/infos`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountInfoData {
    #[serde(default)]
    pub(crate) account_type: Option<u8>,
    #[serde(default)]
    pub(crate) email: Option<String>,
    #[serde(default)]
    pub(crate) pseudo: Option<String>,
}

/// One entry of `value` of `GET /downloader/hosts`; `status`/`is_free`/`name` are fetched but
/// unused (see the IMPL-VERIFY note on `api.rs`'s module doc — `hosters()` returns the flattened
/// domain catalogue, unfiltered by status, mirroring `alldebrid::merge_hosters`).
#[derive(Deserialize)]
pub(crate) struct HostEntry {
    #[serde(default)]
    pub(crate) domains: Vec<String>,
}

/// Lower-cases, deduplicates and sorts every hoster domain from the `downloader/hosts` array
/// (mirrors `alldebrid::merge_hosters`/`premiumize::services::merge_hosters`).
pub(crate) fn merge_hosters(entries: Vec<HostEntry>) -> Vec<String> {
    let mut hosters: Vec<String> = entries
        .into_iter()
        .flat_map(|entry| entry.domains)
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    hosters.sort();
    hosters.dedup();
    hosters
}

/// `true` for `accountType` `1` (premium) or `2` (lifetime); `false` for `0` (free) or an
/// unrecognized value (mirrors JD's `default:` branch treating unknown types as non-premium).
pub(crate) fn is_premium(account_type: Option<u8>) -> bool {
    matches!(account_type, Some(1) | Some(2))
}

/// The name the account label shows: the Debrid-Link `pseudo`, falling back to the account
/// `email`; `None` when the API states neither, and the label then says nothing.
pub(crate) fn account_name<'a>(pseudo: Option<&'a str>, email: Option<&'a str>) -> Option<&'a str> {
    pseudo
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| email.map(str::trim).filter(|value| !value.is_empty()))
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
    #[allow(dead_code)] // `debridlink.api_key_missing` is raised directly by the adapters'
    // `require_secret` gate (`FailureKind::AuthRequired`, bypassing `classify_error` entirely),
    // not by any provider error key — no JD-known key maps to this variant.
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    #[allow(dead_code)] // Debrid-Link's JSON API never challenges with a captcha.
    NeedsCaptcha,
    #[allow(dead_code)] // Kept for `ErrorKind` parity with the other multihoster plugins.
    Unsupported,
}

/// Attaches the provider's raw error key as an `api_code` param so the UI can tell which specific
/// key triggered a shared code/message bucket (e.g. `notDebrid` vs `hostNotValid`, both
/// `debridlink.host_unsupported`).
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

/// Classifies a `{"success":false,"error":"<key>"}` envelope by the provider's error key. Every
/// branch mirrors JDownloader's `DebridLinkCom#errHandling` (`https://debrid-link.com/api_doc/v2/errors`)
/// — see `api.rs`'s module-level IMPL-VERIFY note for the two documented deviations from the task
/// brief.
fn classify_error(code: &str) -> ApiFailure {
    match code {
        // JD's `accountErrorsPermanent`.
        "badToken" => {
            coded_with_provider_code(ErrorKind::AccountInvalid, messages::AUTH_INVALID, code)
        }
        "fileNotFound" => {
            coded_with_provider_code(ErrorKind::Offline, messages::FILE_OFFLINE, code)
        }
        // JD's `downloadErrorsHostUnavailable`, split into two brief-named buckets that share the
        // same `Transient{300}` kind (JD's own `mhm.putError(..., 5 * 60 * 1000l, ...)`).
        "notDebrid" | "hostNotValid" | "notFreeHost" => coded_with_provider_code(
            ErrorKind::Transient(Some(300)),
            messages::HOST_UNSUPPORTED,
            code,
        ),
        "freeServerOverload" | "serverNotAllowed" | "maintenanceHost" | "noServerHost"
        | "disabledServerHost" | "accountLocked" => {
            coded_with_provider_code(ErrorKind::Transient(Some(300)), messages::SERVER_BUSY, code)
        }
        "maxLink" | "maxLinkHost" | "maxData" | "maxDataHost" => coded_with_provider_code(
            ErrorKind::RateLimited(Some(3600)),
            messages::LIMIT_REACHED,
            code,
        ),
        // Deviation from the brief's `RateLimited{600}`: JD's own message for this key documents
        // a 1-hour reset window (see the module doc's IMPL-VERIFY note).
        "floodDetected" => {
            coded_with_provider_code(ErrorKind::RateLimited(Some(3600)), messages::FLOOD, code)
        }
        _ => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(code),
            params: vec![("api_code", code.to_owned())],
        },
    }
}

/// Checks an envelope's `success`/`error` pair; `None` for a `success: true` envelope.
pub(crate) fn error_from_status(success: bool, error: Option<&str>) -> Option<ApiFailure> {
    if success {
        return None;
    }
    Some(classify_error(error.unwrap_or("unknown")))
}

/// Maps an HTTP status the JSON envelope doesn't otherwise explain.
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(ApiFailure {
            kind: ErrorKind::AccountInvalid,
            code: messages::AUTH_INVALID.0,
            message: messages::AUTH_INVALID.1.to_owned(),
            params: Vec::new(),
        }),
        404 | 410 | 451 => Err(ApiFailure {
            kind: ErrorKind::Offline,
            code: messages::FILE_OFFLINE.0,
            message: messages::FILE_OFFLINE.1.to_owned(),
            params: Vec::new(),
        }),
        429 => Err(ApiFailure {
            kind: ErrorKind::RateLimited(None),
            code: messages::RATE_LIMITED.0,
            message: messages::RATE_LIMITED.1.to_owned(),
            params: Vec::new(),
        }),
        500..=599 => Err(ApiFailure {
            kind: ErrorKind::Transient(None),
            code: messages::SERVER_ERROR.0,
            message: messages::SERVER_ERROR.1.to_owned(),
            params: Vec::new(),
        }),
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
    fn add_body_encodes_the_url_form_field() {
        assert_eq!(
            add_body("https://example.test/f/abc"),
            b"url=https%3A%2F%2Fexample.test%2Ff%2Fabc"
        );
    }

    #[test]
    fn parse_download_url_accepts_a_well_formed_url() {
        let parsed = parse_download_url("https://cache.debrid-link.com/dl/tok/release.rar")
            .expect("valid URL");
        assert_eq!(
            parsed.as_str(),
            "https://cache.debrid-link.com/dl/tok/release.rar"
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
    fn error_from_status_requires_an_explicit_failure() {
        assert!(error_from_status(true, None).is_none());
        assert!(error_from_status(false, Some("badToken")).is_some());
    }

    #[test]
    fn classify_error_maps_bad_token_to_account_invalid() {
        let failure = classify_error("badToken");
        assert!(matches!(failure.kind, ErrorKind::AccountInvalid));
        assert_eq!(failure.code, messages::AUTH_INVALID.0);
        assert!(
            failure
                .params
                .iter()
                .any(|(name, value)| *name == "api_code" && value == "badToken")
        );
    }

    #[test]
    fn classify_error_maps_file_not_found_to_offline() {
        let failure = classify_error("fileNotFound");
        assert!(matches!(failure.kind, ErrorKind::Offline));
        assert_eq!(failure.code, messages::FILE_OFFLINE.0);
    }

    #[test]
    fn classify_error_maps_host_unavailable_group_to_transient_300() {
        for code in ["notDebrid", "hostNotValid", "notFreeHost"] {
            let failure = classify_error(code);
            assert!(
                matches!(failure.kind, ErrorKind::Transient(Some(300))),
                "{code}"
            );
            assert_eq!(failure.code, messages::HOST_UNSUPPORTED.0, "{code}");
        }
    }

    #[test]
    fn classify_error_maps_server_busy_group_to_transient_300() {
        for code in [
            "freeServerOverload",
            "serverNotAllowed",
            "maintenanceHost",
            "noServerHost",
            "disabledServerHost",
            "accountLocked",
        ] {
            let failure = classify_error(code);
            assert!(
                matches!(failure.kind, ErrorKind::Transient(Some(300))),
                "{code}"
            );
            assert_eq!(failure.code, messages::SERVER_BUSY.0, "{code}");
        }
    }

    #[test]
    fn classify_error_maps_limit_group_to_rate_limited_3600() {
        for code in ["maxLink", "maxLinkHost", "maxData", "maxDataHost"] {
            let failure = classify_error(code);
            assert!(
                matches!(failure.kind, ErrorKind::RateLimited(Some(3600))),
                "{code}"
            );
            assert_eq!(failure.code, messages::LIMIT_REACHED.0, "{code}");
        }
    }

    #[test]
    fn classify_error_maps_flood_detected_to_rate_limited_3600() {
        let failure = classify_error("floodDetected");
        assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(3600))));
        assert_eq!(failure.code, messages::FLOOD.0);
    }

    #[test]
    fn classify_error_maps_unknown_codes_to_the_generic_bucket() {
        let failure = classify_error("someBrandNewKey");
        assert!(matches!(failure.kind, ErrorKind::Permanent));
        assert_eq!(failure.code, messages::API_ERROR);
        assert!(failure.message.contains("someBrandNewKey"));
        assert!(
            failure
                .params
                .iter()
                .any(|(name, value)| *name == "api_code" && value == "someBrandNewKey")
        );
    }

    #[test]
    fn is_premium_treats_free_and_unknown_as_false() {
        assert!(!is_premium(Some(0)));
        assert!(is_premium(Some(1)));
        assert!(is_premium(Some(2)));
        assert!(!is_premium(None));
        assert!(!is_premium(Some(9)));
    }

    #[test]
    fn merge_hosters_flattens_dedupes_and_sorts() {
        let entries: Vec<HostEntry> = serde_json::from_str(
            r#"[
                {"name": "1fichier", "status": 1, "isFree": true, "domains": ["1fichier.com", "1FICHIER.COM"]},
                {"name": "rapidgator", "status": 1, "isFree": false, "domains": ["rapidgator.net"]},
                {"name": "youtube", "status": 1, "isFree": true, "domains": ["youtube.com", " "]}
            ]"#,
        )
        .expect("parse");
        assert_eq!(
            merge_hosters(entries),
            vec!["1fichier.com", "rapidgator.net", "youtube.com"]
        );
    }

    #[test]
    fn account_name_prefers_pseudo_then_email_then_nothing() {
        assert_eq!(account_name(Some("alice"), Some("a@test")), Some("alice"));
        assert_eq!(account_name(Some("  "), Some("a@test")), Some("a@test"));
        assert_eq!(account_name(None, Some("a@test")), Some("a@test"));
        assert_eq!(account_name(None, None), None);
        assert_eq!(account_name(Some(""), Some("")), None);
    }
}
