//! Target-independent XFS API primitives: file-code extraction, the JSON envelope every XFS
//! `/api/...` endpoint answers with, and its status-code classification. Generalized verbatim from
//! `plugins/ddownload/src/lib.rs`'s pre-Task-11 native module (`file_code`, `FlexibleU64`,
//! `ApiEnvelope`, `ensure_http_status`, `ensure_api_status`) — every classification branch below
//! reproduces ddownload's original mapping exactly (see the IMPL-VERIFY note in
//! `plugins/katfile/src/native/api.rs`'s module doc for the ways KatFile's shape differs, namely
//! its API base path).
//!
//! A consuming plugin owns its own `Failure` type (`rd_core::Failure` natively, the
//! `wit_bindgen`-generated type under `wasm32`) and stable `code`/message text, so this module
//! only classifies: [`ErrorKind`] is deliberately a smaller, target-neutral stand-in for
//! `rd_core::FailureKind`, and callers convert it to their own failure type right after calling in
//! here (see `plugins/ddownload/src/native/api.rs`'s `convert_kind`).

use serde::Deserialize;
use url::Url;

/// Extracts the file code from an XFS file link's first non-empty path segment, restricted to
/// `hosts` (exact, case-sensitive match against `Url::host_str`, mirroring ddownload's original
/// `matches!(host, "ddownload.com" | "www.ddownload.com")`). An XFS file code is at least 6
/// case-sensitive ASCII alphanumeric characters.
#[must_use]
pub fn file_code<'a>(url: &'a Url, hosts: &[&str]) -> Option<&'a str> {
    let host = url.host_str()?;
    // A leading `www.` is the same site, and every XFS installation serves both. Stripping it
    // here rather than asking each caller to list both spellings keeps a host list half as long
    // and, more to the point, removes a way for the two spellings to drift apart. Callers that
    // do list both — ddownload did before this — keep working: this only ever adds a match.
    let host = host.strip_prefix("www.").unwrap_or(host);
    if !hosts.contains(&host) {
        return None;
    }
    let code = url.path_segments()?.find(|segment| !segment.is_empty())?;
    (code.len() >= 6
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric()))
    .then_some(code)
}

/// Some XFS installations report numeric fields (e.g. `traffic_left`, a direct-link `size`) as a
/// JSON number on one endpoint and as a numeric string on another; this accepts either.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum FlexibleU64 {
    Number(u64),
    Text(String),
}

impl FlexibleU64 {
    #[must_use]
    pub fn into_u64(self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

/// Converts an XFS `traffic_left` field into bytes.
///
/// **The XFS account endpoint reports remaining traffic in megabytes**, while `AccountStatus`
/// carries bytes. Passing the number through unchanged turned an account with 112 GiB left into
/// "112 KiB" in the interface — 114688 MB read as 114688 bytes. The conversion lives here rather
/// than in each plugin because it is a property of the XFS API, not of one hoster.
#[must_use]
pub fn traffic_left_bytes(megabytes: Option<FlexibleU64>) -> Option<u64> {
    megabytes
        .and_then(FlexibleU64::into_u64)
        .and_then(|value| value.checked_mul(1024 * 1024))
}

/// XFS API envelope; error responses (e.g. `{"status":400,"msg":"Invalid key"}`) omit `result`.
#[derive(Deserialize)]
pub struct ApiEnvelope<T> {
    pub status: u16,
    #[serde(default)]
    pub msg: String,
    pub result: Option<T>,
}

/// Target-neutral classification of an XFS status code; `None` from [`classify_http_status`] /
/// [`classify_api_status`] means success. Every consuming plugin maps each variant to its own
/// `FailureKind` (native or WIT-generated); see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    AccountInvalid,
    Permanent,
    RateLimited,
    Transient,
}

/// Classifies a raw HTTP transport status (not the API envelope's own `status` field — see
/// [`classify_api_status`] for that). Mirrors ddownload's original `ensure_http_status` mapping
/// exactly, including 404/410/451 -> `Permanent` (not `Offline`; ddownload's transfer flow finds
/// unavailable files through the API envelope, not the raw HTTP status, and Task 11 preserves this
/// as ddownload's living behavior rather than JD's more granular per-page marker).
#[must_use]
pub fn classify_http_status(status: u16) -> Option<ErrorKind> {
    match status {
        200 | 206 => None,
        401 | 403 => Some(ErrorKind::AccountInvalid),
        404 | 410 | 451 => Some(ErrorKind::Permanent),
        429 => Some(ErrorKind::RateLimited),
        500..=599 => Some(ErrorKind::Transient),
        _ => Some(ErrorKind::Permanent),
    }
}

/// Classifies an [`ApiEnvelope`]'s `status` field. Mirrors ddownload's original
/// `ensure_api_status`: 200 is success, 401/403 is `AccountInvalid`, anything else is `Permanent`.
#[must_use]
pub fn classify_api_status(status: u16) -> Option<ErrorKind> {
    if status == 200 {
        None
    } else if matches!(status, 401 | 403) {
        Some(ErrorKind::AccountInvalid)
    } else {
        Some(ErrorKind::Permanent)
    }
}

/// Failure reported while unwrapping an [`ApiEnvelope`] via [`ApiEnvelope::into_result`].
pub enum EnvelopeError {
    /// The envelope's `status` classified as an error; carries its `msg` (the provider message).
    Status(ErrorKind, String),
    /// `status` was 200 (success) but `result` was absent — an XFS response that does not match
    /// its own documented shape.
    MissingResult,
}

impl<T> ApiEnvelope<T> {
    /// Unwraps a successful envelope's `result`, or reports why it couldn't.
    pub fn into_result(self) -> Result<T, EnvelopeError> {
        if let Some(kind) = classify_api_status(self.status) {
            return Err(EnvelopeError::Status(kind, self.msg));
        }
        self.result.ok_or(EnvelopeError::MissingResult)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiEnvelope, EnvelopeError, ErrorKind, FlexibleU64, classify_api_status,
        classify_http_status, file_code,
    };

    const HOSTS: &[&str] = &["ddownload.com", "www.ddownload.com"];

    #[test]
    fn file_code_requires_a_matching_host_and_a_six_character_alnum_first_segment() {
        let url = "https://ddownload.com/abc123xyz/release.rar"
            .parse()
            .expect("URL");
        assert_eq!(file_code(&url, HOSTS), Some("abc123xyz"));

        let too_short = "https://ddownload.com/ab12".parse().expect("URL");
        assert_eq!(file_code(&too_short, HOSTS), None);

        let wrong_host = "https://fs7.ddownload.com/abc123xyz".parse().expect("URL");
        assert_eq!(file_code(&wrong_host, HOSTS), None);

        let other_site = "https://example.com/abc123xyz".parse().expect("URL");
        assert_eq!(file_code(&other_site, HOSTS), None);
    }

    #[test]
    fn flexible_u64_parses_both_number_and_text_json() {
        let number: FlexibleU64 = serde_json::from_str("42").expect("number");
        assert_eq!(number.into_u64(), Some(42));

        let text: FlexibleU64 = serde_json::from_str("\"4096\"").expect("text");
        assert_eq!(text.into_u64(), Some(4096));

        let invalid: FlexibleU64 = serde_json::from_str("\"not-a-number\"").expect("text");
        assert_eq!(invalid.into_u64(), None);
    }

    #[test]
    fn classify_http_status_mirrors_ddownloads_original_mapping() {
        assert_eq!(classify_http_status(200), None);
        assert_eq!(classify_http_status(206), None);
        assert_eq!(classify_http_status(401), Some(ErrorKind::AccountInvalid));
        assert_eq!(classify_http_status(403), Some(ErrorKind::AccountInvalid));
        assert_eq!(classify_http_status(404), Some(ErrorKind::Permanent));
        assert_eq!(classify_http_status(410), Some(ErrorKind::Permanent));
        assert_eq!(classify_http_status(451), Some(ErrorKind::Permanent));
        assert_eq!(classify_http_status(429), Some(ErrorKind::RateLimited));
        assert_eq!(classify_http_status(500), Some(ErrorKind::Transient));
        assert_eq!(classify_http_status(599), Some(ErrorKind::Transient));
        assert_eq!(classify_http_status(999), Some(ErrorKind::Permanent));
    }

    #[test]
    fn classify_api_status_only_ever_reports_account_invalid_or_permanent() {
        assert_eq!(classify_api_status(200), None);
        assert_eq!(classify_api_status(401), Some(ErrorKind::AccountInvalid));
        assert_eq!(classify_api_status(403), Some(ErrorKind::AccountInvalid));
        assert_eq!(classify_api_status(400), Some(ErrorKind::Permanent));
        assert_eq!(classify_api_status(404), Some(ErrorKind::Permanent));
    }

    #[test]
    fn envelope_into_result_reports_status_error_before_missing_result() {
        let error: ApiEnvelope<u32> =
            serde_json::from_str(r#"{"status":400,"msg":"Invalid key"}"#).expect("json");
        match error.into_result() {
            Err(EnvelopeError::Status(ErrorKind::Permanent, message)) => {
                assert_eq!(message, "Invalid key")
            }
            _ => panic!("expected a status error, got a different outcome"),
        }

        let missing: ApiEnvelope<u32> =
            serde_json::from_str(r#"{"status":200,"msg":"OK"}"#).expect("json");
        assert!(matches!(
            missing.into_result(),
            Err(EnvelopeError::MissingResult)
        ));

        let ok: ApiEnvelope<u32> =
            serde_json::from_str(r#"{"status":200,"msg":"OK","result":7}"#).expect("json");
        assert_eq!(ok.into_result().ok(), Some(7));
    }
}

/// Parses `YYYY-MM-DD HH:MM:SS` (UTC) into seconds since the Unix epoch.
///
/// Hand-rolled rather than delegated to a date library: this runs in the WebAssembly guest as
/// well, where chrono would pull a JavaScript clock binding that the sandbox has no way to
/// satisfy. Only the parse is needed here — the current time comes from the host.
#[must_use]
pub fn parse_expiry_unix(value: &str) -> Option<i64> {
    let (date, time) = value.trim().split_once(' ')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next().unwrap_or("0").parse().ok()?;
    if time.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days between 1970-01-01 and the given date, by Howard Hinnant's civil-calendar algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod expiry_tests {
    use super::parse_expiry_unix;

    #[test]
    fn a_provider_expiry_becomes_a_comparable_instant() {
        assert_eq!(parse_expiry_unix("1970-01-01 00:00:00"), Some(0));
        assert_eq!(
            parse_expiry_unix("2026-09-04 12:00:00"),
            Some(1_788_523_200)
        );
        // A leap day, because the whole point of the algorithm is getting those right.
        assert_eq!(
            parse_expiry_unix("2024-02-29 00:00:00"),
            Some(1_709_164_800)
        );
    }

    #[test]
    fn an_unreadable_expiry_is_none_rather_than_a_guess() {
        for value in [
            "",
            "soon",
            "2026-13-01 00:00:00",
            "2026-09-04",
            "2026-09-04 25:00:00",
        ] {
            assert_eq!(parse_expiry_unix(value), None, "{value}");
        }
    }

    use super::file_code;

    fn url(value: &str) -> url::Url {
        value.parse().expect("url")
    }

    #[test]
    fn a_www_prefix_is_the_same_site() {
        let hosts = ["hexload.com"];
        assert_eq!(
            file_code(&url("https://hexload.com/abc123xyz"), &hosts),
            Some("abc123xyz")
        );
        assert_eq!(
            file_code(&url("https://www.hexload.com/abc123xyz"), &hosts),
            Some("abc123xyz")
        );
    }

    /// Stripping the prefix must not turn a lookalike into a match: only a literal leading
    /// `www.` is removed, and what remains still has to be in the list.
    #[test]
    fn a_lookalike_host_is_still_refused() {
        let hosts = ["hexload.com"];
        for host in [
            "https://hexload.com.evil.test/abc123xyz",
            "https://wwwhexload.com/abc123xyz",
            "https://www.hexload.com.evil.test/abc123xyz",
            "https://cdn.hexload.com/abc123xyz",
        ] {
            assert_eq!(file_code(&url(host), &hosts), None, "{host}");
        }
    }

    /// A caller that still lists both spellings keeps working unchanged.
    #[test]
    fn listing_both_spellings_remains_valid() {
        let hosts = ["ddownload.com", "www.ddownload.com"];
        assert_eq!(
            file_code(&url("https://www.ddownload.com/abc123xyz"), &hosts),
            Some("abc123xyz")
        );
    }
}

/// Why the optional `file/direct_link` endpoint produced no link.
///
/// No XFS installation documents that endpoint, so a plugin that asks for it has to treat the
/// answer as optional and fall back to the cookie-backed premium flow. Until RD-120-13 the
/// fallback was also *silent* — four `.ok()?` in a row — and the running installation's error
/// log said nothing at all about the provider while a user spent an evening on it. The reason
/// is named now, as one of these five fixed phrases: a constant cannot carry a file code, a
/// URL or an API key into a log line, and interpolating the provider's own answer is exactly
/// how one would get there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectLinkSkip {
    /// The request did not come back at all.
    RequestFailed,
    /// The answer was not the JSON envelope the endpoint is supposed to return.
    NotJson,
    /// The envelope carried an error instead of a result.
    ApiError,
    /// The link in the result does not parse as a URL.
    UnparsableUrl,
    /// The link points at a host that is not the provider's own.
    ForeignHost,
}

impl DirectLinkSkip {
    /// The fixed phrase this reason contributes to the log line.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::RequestFailed => "the request failed",
            Self::NotJson => "the answer was not the documented JSON envelope",
            Self::ApiError => "the API answered with an error",
            Self::UnparsableUrl => "the link it returned is not a URL",
            Self::ForeignHost => "the link it returned points at another host",
        }
    }
}

/// The one line a silent fallback leaves behind.
///
/// `provider` is the plugin's own display name, never a value that came off the wire.
#[must_use]
pub fn direct_link_skipped(provider: &str, skip: DirectLinkSkip) -> String {
    format!(
        "{provider}: the API direct link was not used ({}), falling back to the cookie premium \
         flow",
        skip.reason()
    )
}

#[cfg(test)]
mod direct_link_skip_tests {
    use super::{DirectLinkSkip, direct_link_skipped};

    const ALL: [DirectLinkSkip; 5] = [
        DirectLinkSkip::RequestFailed,
        DirectLinkSkip::NotJson,
        DirectLinkSkip::ApiError,
        DirectLinkSkip::UnparsableUrl,
        DirectLinkSkip::ForeignHost,
    ];

    /// Five reasons, five distinguishable lines: a log that cannot tell them apart is the
    /// silent fallback again, only louder.
    #[test]
    fn every_reason_reads_differently() {
        let mut lines: Vec<String> = ALL
            .iter()
            .map(|skip| direct_link_skipped("DDownload", *skip))
            .collect();
        lines.sort();
        lines.dedup();
        assert_eq!(lines.len(), ALL.len());
    }

    /// The line names the provider and the fallback, and nothing else can reach it: the only
    /// variable part is the caller's own display name.
    #[test]
    fn the_line_carries_nothing_that_came_off_the_wire() {
        for skip in ALL {
            let line = direct_link_skipped("KatFile", skip);
            assert!(line.starts_with("KatFile: "), "{line}");
            assert!(
                line.contains("falling back to the cookie premium flow"),
                "{line}"
            );
            assert!(!line.contains("://"), "no address may travel in it: {line}");
            assert!(
                !line.contains('='),
                "no query parameter may travel in it: {line}"
            );
        }
    }
}
