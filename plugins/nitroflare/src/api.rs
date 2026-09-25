//! Target-independent Nitroflare API v2 logic: URL matching, response shapes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! IMPL-VERIFY (against JD's `NitroFlareCom.java`, the living reference — see
//! task-7-report.md for the full list):
//! - The file-id pattern is `/(?:view|watch)/([A-Z0-9]+)` — **uppercase** letters and digits
//!   only, and either `/view/` or `/watch/` (the brief only mentioned `/view/`).
//! - `getDownloadLink`'s response carries only `result.url`; unlike the brief's assumption,
//!   name/size never come from it. JD instead reads them from a preceding `getFileInfo` call
//!   (`requestFileInformationAPI`, called right before `handlePremiumDownloadAPI`) — mirrored
//!   here by `resolve()` calling `getFileInfo` before `getDownloadLink`, the same two-call shape
//!   `plugins/onefichier` uses (`file/info.cgi` then `download/get_token.cgi`).
//! - Envelopes are classified by a top-level `code` field (`checkErrorsAPI`), not by a `"type"`
//!   discriminator as the brief described: `code` absent or `-1` is success; any other value is
//!   an error, `message` carrying the provider's text. `checkErrorsAPI`'s `switch` has exactly
//!   six arms — `-1`, `1`, `4`, `6`, `8`, `12`, plus `default` — and this is the complete list:
//!   `1` = access denied (premium-only file / non-premium account), `4` = file doesn't exist,
//!   `6` = invalid captcha response (JD's own comment: "This should rarely/never happen!!" —
//!   only reachable after JD itself submits a captcha solution, which this plugin never does),
//!   `8` = invalid login data, `12` = the API itself now demands a captcha to continue (abuse
//!   protection). This plugin has no captcha-solving pipeline, so both `6` and `12` map to
//!   `FailureKind::NeedsCaptcha` (with distinct codes/messages — "invalid" vs. "required") rather
//!   than the generic bucket. Every other `code` value falls into JD's `default` arm.
//! - JD's `checkErrorsAPI` switch has no dedicated code for traffic/bandwidth exhaustion (that
//!   phrasing only appears in JD's *website*-mode HTML error strings, which this pure-API
//!   plugin never sees). The brief's "traffic exhausted → RateLimited{3600}" mapping is
//!   approximated here by matching the generic-error `message` text for traffic/bandwidth
//!   wording, mirroring those website-mode phrases as closely as the API surface allows.
//! - `getFileInfo`'s `result.files` is a map keyed by file id — *unless* every requested id was
//!   invalid, in which case JD documents it comes back as an empty JSON array instead of an
//!   empty object. Handled by [`deserialize_files`].

use std::collections::HashMap;

use serde::Deserialize;
use url::Url;

use crate::messages;

/// Bare hostname (no `www.` prefix) this hoster's file links carry. JD's plugin also accepts
/// the `nitroflare.net` and `nitro.download` aliases, but the task brief pins `match_domains` to
/// `nitroflare.com` only — see task-7-report.md.
pub(crate) const MATCH_HOST: &str = "nitroflare.com";

pub(crate) const API_BASE: &str = "https://nitroflare.com/api/v2";

/// Extracts the file id from a Nitroflare link, e.g. `https://nitroflare.com/view/ABC123DEF`.
///
/// Mirrors JD's `PATTERN_FILE = "/(?:view|watch)/([A-Z0-9]+)"`: the host (`www.`-stripped) must
/// be [`MATCH_HOST`], the path must start with `/view/` or `/watch/`, and the id is the longest
/// run of uppercase-ASCII-alphanumeric characters that follows (JD's regex is not anchored at
/// the end, so trailing path segments are tolerated, matching `1fichier::file_id`'s handling of
/// a trailing query string).
pub(crate) fn file_id(url: &Url) -> Option<&str> {
    let host_str = url.host_str()?;
    let host = host_str.strip_prefix("www.").unwrap_or(host_str);
    if host != MATCH_HOST {
        return None;
    }
    let path = url.path();
    let rest = path
        .strip_prefix("/view/")
        .or_else(|| path.strip_prefix("/watch/"))?;
    let end = rest
        .char_indices()
        .find(|(_, character)| !(character.is_ascii_uppercase() || character.is_ascii_digit()))
        .map_or(rest.len(), |(index, _)| index);
    let id = &rest[..end];
    (!id.is_empty()).then_some(id)
}

/// Validates the raw download URL string `getDownloadLink`'s `result.url` carries. Shared so a
/// malformed URL from the API produces the exact same `nitroflare.invalid_url` failure on both
/// the native and guest adapters instead of one erroring and the other silently forwarding a
/// URL that later fails to parse deeper in the pipeline (or not at all, on the guest side, where
/// `ResolvedDownload.url` is a bare `String`).
pub(crate) fn parse_download_url(raw: &str) -> Result<Url, ApiFailure> {
    Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// Generic `{"result": {...}}` / `{"message": "...", "code": N}` envelope every Nitroflare v2
/// endpoint answers with (see the module-level IMPL-VERIFY note on how `code` is classified).
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) result: Option<T>,
    pub(crate) code: Option<i64>,
    pub(crate) message: Option<String>,
}

/// `result` of `GET /getDownloadLink`.
#[derive(Deserialize)]
pub(crate) struct DownloadLinkResult {
    pub(crate) url: Option<String>,
}

/// `result` of `GET /getKeyInfo`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyInfoResult {
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) traffic_left: Option<FlexibleU64>,
    /// A date string (`"yyyy-MM-dd HH:mm:ss"`), or `0`/`"0"`/absent for a non-premium key. Kept
    /// as a raw JSON value since JD checks both a numeric and a string `0` (`AccountInfo`'s
    /// `expiryDateO.toString().equals("0")`); see [`expiry_text`].
    #[serde(default)]
    pub(crate) expiry_date: Option<serde_json::Value>,
}

/// `None` for an absent/zero expiry (free key); otherwise the raw date text.
pub(crate) fn expiry_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty() && trimmed != "0").then(|| trimmed.to_owned())
        }
        serde_json::Value::Number(number) => {
            (number.as_i64() != Some(0)).then(|| number.to_string())
        }
        _ => None,
    }
}

/// `result` of `GET /getFileInfo`.
#[derive(Deserialize)]
pub(crate) struct FileInfoResult {
    #[serde(default, deserialize_with = "deserialize_files")]
    pub(crate) files: HashMap<String, FileEntry>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct FileEntry {
    pub(crate) status: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) size: Option<FlexibleU64>,
}

impl FileEntry {
    pub(crate) fn is_online(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("online"))
    }
}

/// `result.files` is a map keyed by file id, except when every requested id was invalid — JD
/// documents that case answers with an empty JSON array instead ("If all given fileIDs are
/// invalid it will be an empty list!").
fn deserialize_files<'de, D>(deserializer: D) -> Result<HashMap<String, FileEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Files {
        Map(HashMap<String, FileEntry>),
        // The value itself is never read; only the variant's shape (a JSON array) matters.
        EmptyList(#[allow(dead_code)] Vec<serde_json::Value>),
    }
    Ok(match Files::deserialize(deserializer)? {
        Files::Map(map) => map,
        Files::EmptyList(_) => HashMap::new(),
    })
}

/// Nitroflare reports traffic in bytes as either a JSON number or a numeric string.
#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum FlexibleU64 {
    Number(u64),
    Text(String),
}

impl FlexibleU64 {
    pub(crate) fn into_u64(self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(value),
            Self::Text(value) => value.parse().ok(),
        }
    }
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
    NeedsCaptcha,
    #[allow(dead_code)] // `matches()` filters unsupported links before any call is made.
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

/// Classifies a non-success `code`/`message` pair from the envelope (see the module-level
/// IMPL-VERIFY note). Covers every arm of JD's `checkErrorsAPI` switch (`1`, `4`, `6`, `8`,
/// `12`); `code` values not in that switch fall into JD's own `default` arm here, further split
/// by a traffic/bandwidth-exhaustion heuristic on `message` (see the note above on why there is
/// no dedicated numeric code for it in API mode).
fn classify_code(code: i64, message: &str) -> ApiFailure {
    match code {
        1 => coded(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED),
        4 => coded(ErrorKind::Offline, messages::FILE_OFFLINE),
        6 => coded(ErrorKind::NeedsCaptcha, messages::CAPTCHA_INVALID),
        8 => coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS),
        12 => coded(ErrorKind::NeedsCaptcha, messages::CAPTCHA_REQUIRED),
        _ if is_traffic_exhausted(message) => coded(
            ErrorKind::RateLimited(Some(3600)),
            messages::TRAFFIC_EXHAUSTED,
        ),
        _ => ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(code, message),
            params: vec![
                ("api_code", code.to_string()),
                ("message", message.to_owned()),
            ],
        },
    }
}

/// Matches the traffic/bandwidth-exhaustion wording JD's website-mode error strings use
/// (`"reached the maximum volume for today"`, `"exceeds the daily download limit"`) — the
/// closest verified phrasing available, since API mode has no dedicated error code for this.
/// `"daily download"` is checked on its own since JD's "exceeds the daily download limit"
/// phrase carries no traffic/volume/bandwidth keyword.
fn is_traffic_exhausted(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let mentions_traffic = lower.contains("traffic")
        || lower.contains("volume")
        || lower.contains("bandwidth")
        || lower.contains("daily download");
    let mentions_exhaustion =
        lower.contains("exceed") || lower.contains("limit") || lower.contains("maximum");
    mentions_traffic && mentions_exhaustion
}

/// Checks an envelope's `code`/`message` pair; `None` for success (`code` absent or `-1`).
pub(crate) fn error_from_envelope(code: Option<i64>, message: Option<&str>) -> Option<ApiFailure> {
    let code = code?;
    if code == -1 {
        return None;
    }
    Some(classify_code(
        code,
        message.unwrap_or("Unknown Nitroflare API error"),
    ))
}

/// Maps an HTTP status the JSON envelope doesn't otherwise explain.
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS)),
        404 | 410 | 451 => Err(coded(ErrorKind::Offline, messages::FILE_OFFLINE)),
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

    fn url(value: &str) -> Url {
        value.parse().expect("URL")
    }

    #[test]
    fn file_id_accepts_view_and_watch_paths() {
        assert_eq!(
            file_id(&url("https://nitroflare.com/view/ABCDEFGHIJ")),
            Some("ABCDEFGHIJ")
        );
        assert_eq!(
            file_id(&url("https://www.nitroflare.com/view/ABCDEFGHIJ")),
            Some("ABCDEFGHIJ")
        );
        assert_eq!(
            file_id(&url("https://nitroflare.com/watch/ABCDEFGHIJ")),
            Some("ABCDEFGHIJ")
        );
    }

    #[test]
    fn file_id_tolerates_trailing_path_segments() {
        assert_eq!(
            file_id(&url("https://nitroflare.com/view/ABCDEFGHIJ/movie-name")),
            Some("ABCDEFGHIJ")
        );
    }

    #[test]
    fn file_id_rejects_lowercase_and_unsupported_shapes() {
        // JD's pattern is strictly `[A-Z0-9]+` — lowercase ids never matched the live site.
        assert_eq!(
            file_id(&url("https://nitroflare.com/view/abcdefghij")),
            None
        );
        assert_eq!(file_id(&url("https://nitroflare.com/")), None);
        assert_eq!(file_id(&url("https://nitroflare.com/member?s=api")), None);
        assert_eq!(file_id(&url("https://evil.example/view/ABCDEFGHIJ")), None);
    }

    #[test]
    fn parse_download_url_accepts_a_well_formed_url() {
        let parsed =
            parse_download_url("https://cdn1.nitroflare.com/d/tok/release.rar").expect("valid URL");
        assert_eq!(
            parsed.as_str(),
            "https://cdn1.nitroflare.com/d/tok/release.rar"
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
    fn error_from_envelope_treats_absent_and_negative_one_code_as_success() {
        assert!(error_from_envelope(None, None).is_none());
        assert!(error_from_envelope(Some(-1), None).is_none());
        assert!(error_from_envelope(Some(8), Some("Wrong login")).is_some());
    }

    #[test]
    fn classify_code_maps_known_codes() {
        let premium_required = classify_code(1, "Access denied");
        assert!(matches!(premium_required.kind, ErrorKind::AuthRequired));
        assert_eq!(premium_required.code, messages::PREMIUM_REQUIRED.0);

        let offline = classify_code(4, "File doesn't exist");
        assert!(matches!(offline.kind, ErrorKind::Offline));
        assert_eq!(offline.code, messages::FILE_OFFLINE.0);

        let invalid_captcha = classify_code(6, "Invalid captcha");
        assert!(matches!(invalid_captcha.kind, ErrorKind::NeedsCaptcha));
        assert_eq!(invalid_captcha.code, messages::CAPTCHA_INVALID.0);
        // Distinct from code 12's "required" wording, even though both are NeedsCaptcha.
        assert_ne!(invalid_captcha.code, messages::CAPTCHA_REQUIRED.0);

        let bad_credentials = classify_code(8, "Wrong login");
        assert!(matches!(bad_credentials.kind, ErrorKind::AccountInvalid));
        assert_eq!(bad_credentials.code, messages::BAD_CREDENTIALS.0);

        let captcha = classify_code(12, "Captcha required");
        assert!(matches!(captcha.kind, ErrorKind::NeedsCaptcha));
        assert_eq!(captcha.code, messages::CAPTCHA_REQUIRED.0);
    }

    #[test]
    fn classify_code_maps_traffic_wording_in_the_generic_bucket() {
        let failure = classify_code(99, "You have exceeded your daily traffic limit");
        assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(3600))));
        assert_eq!(failure.code, messages::TRAFFIC_EXHAUSTED.0);
    }

    #[test]
    fn classify_code_maps_the_daily_download_limit_phrase_without_a_traffic_keyword() {
        // JD's website-mode phrase "This download exceeds the daily download limit" carries no
        // traffic/volume/bandwidth keyword, so it needs its own trigger in `is_traffic_exhausted`.
        let failure = classify_code(99, "This download exceeds the daily download limit");
        assert!(matches!(failure.kind, ErrorKind::RateLimited(Some(3600))));
        assert_eq!(failure.code, messages::TRAFFIC_EXHAUSTED.0);
    }

    #[test]
    fn classify_code_falls_back_to_the_generic_bucket() {
        let failure = classify_code(42, "Some new provider error");
        assert!(matches!(failure.kind, ErrorKind::Permanent));
        assert_eq!(failure.code, messages::API_ERROR);
        assert!(failure.message.contains("Some new provider error"));
        assert!(
            failure
                .params
                .iter()
                .any(|(name, value)| *name == "api_code" && value == "42")
        );
    }

    #[test]
    fn expiry_text_treats_zero_and_absent_as_no_expiry() {
        assert_eq!(expiry_text(&serde_json::json!("0")), None);
        assert_eq!(expiry_text(&serde_json::json!(0)), None);
        assert_eq!(expiry_text(&serde_json::json!("")), None);
        assert_eq!(
            expiry_text(&serde_json::json!("2027-01-01 00:00:00")),
            Some("2027-01-01 00:00:00".to_owned())
        );
    }

    #[test]
    fn deserialize_files_treats_an_empty_list_as_an_empty_map() {
        let result: FileInfoResult = serde_json::from_str(r#"{"files": []}"#).expect("empty list");
        assert!(result.files.is_empty());

        let result: FileInfoResult = serde_json::from_str(
            r#"{"files": {"ABC123": {"status": "online", "name": "a.rar", "size": "10"}}}"#,
        )
        .expect("map");
        assert_eq!(result.files.len(), 1);
        assert!(result.files["ABC123"].is_online());
    }
}
