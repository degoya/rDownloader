//! Target-independent 1fichier API logic: URL matching, request bodies, response shapes and
//! error classification. Shared verbatim by the native (`native.rs`) and WebAssembly
//! (`guest.rs`) adapters so both report byte-identical failure codes and messages; neither
//! `rd-core` nor `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`,
//! which are available on every target.

use serde::Deserialize;
use url::Url;

use crate::messages;

/// Bare hostnames (no `www.` prefix, no wildcards) this hoster's file links carry.
/// Mirrors `manifest.toml`'s `match_domains` (minus the `www.` variants, stripped in
/// [`file_id`]) and `rd-provider-registry`'s `1fichier` row.
pub(crate) const MATCH_HOSTS: &[&str] = &[
    "1fichier.com",
    "alterupload.com",
    "cjoint.net",
    "desfichiers.com",
    "megadl.fr",
    "mesfichiers.org",
    "dl4free.com",
    "tenvoi.com",
    "piecejointe.net",
    "pjointe.com",
];

/// Extracts the file id from a 1fichier link, e.g. `https://1fichier.com/?abc12defg3`.
///
/// Mirrors JDownloader's `OneFichierCom` pattern
/// `https?://(?:www\.)?<host>/\?([a-z0-9]{5,20})`: the host (`www.`-stripped) must be one of
/// [`MATCH_HOSTS`], the path must be exactly `/`, and the query must start with 5-20 lowercase
/// alphanumeric characters (a trailing `&lg=en` or similar is tolerated, matching the
/// non-anchored JD regex).
pub(crate) fn file_id(url: &Url) -> Option<&str> {
    let host_str = url.host_str()?;
    let host = host_str.strip_prefix("www.").unwrap_or(host_str);
    if !MATCH_HOSTS.contains(&host) {
        return None;
    }
    if url.path() != "/" {
        return None;
    }
    let query = url.query()?;
    let end = query
        .char_indices()
        .find(|(_, character)| !(character.is_ascii_lowercase() || character.is_ascii_digit()))
        .map_or(query.len(), |(index, _)| index);
    let id = &query[..end];
    (5..=20).contains(&id.len()).then_some(id)
}

/// Reconstructs the canonical `https://<host>/?<id>` link 1fichier's API expects, preserving
/// the original (possibly aliased, possibly `www.`-prefixed) host — uploaders may restrict a
/// file to one specific domain, so normalizing away from it would make the link offline.
pub(crate) fn canonical_link(url: &Url) -> Option<String> {
    let id = file_id(url)?;
    let host = url.host_str()?;
    Some(format!("https://{host}/?{id}"))
}

/// JSON body `{"url": "<link>"}` used by `file/info.cgi` and `download/get_token.cgi`.
pub(crate) fn link_body(link: &str) -> Vec<u8> {
    serde_json::json!({ "url": link }).to_string().into_bytes()
}

/// Validates the raw download URL string `download/get_token.cgi` returns. Shared so a
/// malformed URL from the API produces the exact same `1fichier.invalid_url` failure on both
/// the native and guest adapters instead of one erroring and the other silently forwarding a
/// URL that later fails to parse deeper in the pipeline (or not at all, on the guest side,
/// where `ResolvedDownload.url` is a bare `String`).
pub(crate) fn parse_download_url(raw: &str) -> Result<Url, ApiFailure> {
    Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// `POST /v1/download/get_token.cgi` response.
#[derive(Deserialize)]
pub(crate) struct GetTokenResponse {
    pub(crate) status: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) url: Option<String>,
}

/// `POST /v1/file/info.cgi` response.
#[derive(Deserialize)]
pub(crate) struct FileInfoResponse {
    pub(crate) status: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) filename: Option<String>,
    pub(crate) size: Option<FlexibleU64>,
}

/// `POST /v1/user/info.cgi` response.
#[derive(Deserialize)]
pub(crate) struct UserInfoResponse {
    pub(crate) status: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) email: Option<String>,
    /// `0` = Free, `1` = Premium, `2` = Access.
    pub(crate) offer: Option<FlexibleU64>,
    pub(crate) subscription_end: Option<String>,
    /// CDN download credits, in gigabytes.
    pub(crate) cdn: Option<FlexibleU64>,
}

/// 1fichier reports several numeric fields as either a JSON number or a numeric string.
#[derive(Deserialize)]
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

/// Mirrors `rd_core::FailureKind` / the WIT `failure-kind` variant, without depending on
/// either.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    #[allow(dead_code)] // 1fichier's JSON API never challenges with a captcha.
    NeedsCaptcha,
    #[allow(dead_code)]
    // Kept for parity with `FailureKind`; `matches()` filters unsupported links earlier.
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

/// Classifies a `{"status":"KO","message":"..."}` envelope by the provider's `message` text.
/// Every branch mirrors JDownloader's `OneFichierCom#handleErrorsAPI` regexes; flood messages
/// never carry an explicit retry delay, so the fixed 5-minute cooldown JD itself applies
/// (`AccountUnavailableException(msg, 5 * 60 * 1000)`) is used instead of parsing one.
fn classify_message(message: &str) -> ApiFailure {
    let lower = message.to_ascii_lowercase();
    if lower.contains("flood detected") {
        coded(ErrorKind::RateLimited(Some(300)), messages::FLOOD)
    } else if lower.contains("not authenticated") || lower.contains("no such user") {
        coded(ErrorKind::AccountInvalid, messages::BAD_API_KEY)
    } else if lower.contains("must be a customer") {
        coded(ErrorKind::AuthRequired, messages::PREMIUM_REQUIRED)
    } else if lower.contains("resource not found") {
        coded(ErrorKind::Offline, messages::FILE_OFFLINE)
    } else {
        ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::API_ERROR,
            message: messages::api_error(message),
            params: vec![("message", message.to_owned())],
        }
    }
}

/// Checks a parsed envelope's `status`/`message` pair. 1fichier only ever sets `"status"` on
/// error responses (`"KO"`); a success response may omit it entirely or answer with a bare
/// `"OK"`, so — following JD's `handleErrorsAPI`, the living reference — anything other than an
/// explicit case-insensitive `"KO"` is treated as success.
pub(crate) fn error_from_status(status: Option<&str>, message: Option<&str>) -> Option<ApiFailure> {
    if !status.is_some_and(|value| value.eq_ignore_ascii_case("KO")) {
        return None;
    }
    Some(classify_message(
        message.unwrap_or("Unknown 1fichier API error"),
    ))
}

/// Maps an HTTP status the JSON envelope doesn't otherwise explain.
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(coded(ErrorKind::AccountInvalid, messages::BAD_API_KEY)),
        404 => Err(coded(ErrorKind::Offline, messages::FILE_OFFLINE)),
        429 => Err(coded(ErrorKind::RateLimited(Some(300)), messages::FLOOD)),
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
    fn file_id_accepts_supported_hosts_and_aliases() {
        assert_eq!(
            file_id(&url("https://1fichier.com/?abc12defg3")),
            Some("abc12defg3")
        );
        assert_eq!(
            file_id(&url("https://www.1fichier.com/?abc12defg3")),
            Some("abc12defg3")
        );
        assert_eq!(
            file_id(&url("https://alterupload.com/?abc12defg3")),
            Some("abc12defg3")
        );
    }

    #[test]
    fn file_id_tolerates_trailing_query_parameters() {
        assert_eq!(
            file_id(&url("https://1fichier.com/?abc12defg3&lg=en")),
            Some("abc12defg3")
        );
    }

    #[test]
    fn file_id_rejects_unsupported_shapes() {
        assert_eq!(file_id(&url("https://1fichier.com/")), None);
        assert_eq!(
            file_id(&url("https://1fichier.com/console/params.pl")),
            None
        );
        assert_eq!(file_id(&url("https://evil.example/?abc12defg3")), None);
        // Too short to be a real id (JD requires 5-20 chars).
        assert_eq!(file_id(&url("https://1fichier.com/?abcd")), None);
    }

    #[test]
    fn canonical_link_preserves_the_original_host() {
        assert_eq!(
            canonical_link(&url("https://alterupload.com/?abc12defg3&lg=en")).as_deref(),
            Some("https://alterupload.com/?abc12defg3")
        );
    }

    #[test]
    fn parse_download_url_accepts_a_well_formed_url() {
        let parsed =
            parse_download_url("https://cdn123.1fichier.com/d/tok/release.rar").expect("valid URL");
        assert_eq!(
            parsed.as_str(),
            "https://cdn123.1fichier.com/d/tok/release.rar"
        );
    }

    #[test]
    fn parse_download_url_rejects_a_malformed_url() {
        // The single tested path both `native.rs` and `guest.rs` delegate to, so a malformed
        // `download/get_token.cgi` response can no longer be handled inconsistently between
        // the two adapters (native used to `Url::parse` and fail; guest used to pass the raw
        // string straight through unchecked).
        let failure = parse_download_url("not a url").expect_err("malformed URL");
        assert!(matches!(failure.kind, ErrorKind::Permanent));
        assert_eq!(failure.code, messages::INVALID_URL);
        assert!(
            failure.message.starts_with("Invalid provider URL:"),
            "{}",
            failure.message
        );
        assert!(
            failure.params.iter().any(|(name, _)| *name == "error"),
            "expected an `error` parameter"
        );
    }

    #[test]
    fn error_from_status_requires_an_explicit_ko() {
        assert!(error_from_status(None, None).is_none());
        assert!(error_from_status(Some("OK"), None).is_none());
        assert!(error_from_status(Some("ko"), Some("Resource not found #1")).is_some());
    }

    #[test]
    fn classify_message_maps_known_patterns() {
        let flood = classify_message("Flood detected: IP Locked #38");
        assert!(matches!(flood.kind, ErrorKind::RateLimited(Some(300))));
        assert_eq!(flood.code, messages::FLOOD.0);

        let auth = classify_message("Not authenticated #12");
        assert!(matches!(auth.kind, ErrorKind::AccountInvalid));
        assert_eq!(auth.code, messages::BAD_API_KEY.0);

        let premium = classify_message("Must be a customer (Premium, Access) #200");
        assert!(matches!(premium.kind, ErrorKind::AuthRequired));
        assert_eq!(premium.code, messages::PREMIUM_REQUIRED.0);

        let offline = classify_message("Resource not found #469");
        assert!(matches!(offline.kind, ErrorKind::Offline));
        assert_eq!(offline.code, messages::FILE_OFFLINE.0);

        let other = classify_message("Owner locked #649");
        assert!(matches!(other.kind, ErrorKind::Permanent));
        assert_eq!(other.code, messages::API_ERROR);
        assert!(other.message.contains("Owner locked #649"));
    }
}
