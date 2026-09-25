use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header};
use url::Url;

use crate::{HttpDownloadError, engine::network_failure, engine::status_failure};

/// Payload size above which a text-ish content type is still accepted as a file: hoster
/// error pages are small, real payloads are not.
const LARGE_PAYLOAD_BYTES: u64 = 2 * 1024 * 1024;

/// Slack allowed between an announced size and the one the server offers, as a percentage of
/// the announcement. Hosters round what they print ("405.44 MB"), so an exact match cannot be
/// required.
const SIZE_TOLERANCE_PERCENT: u64 = 1;

/// Slack allowed regardless of the announced size. It exists so that no small file can ever
/// trip the rule: below this, the two numbers are treated as agreeing whatever they are.
const SIZE_TOLERANCE_FLOOR_BYTES: u64 = 4096;

/// Whether the length a server offers contradicts the size the hoster announced for the file.
///
/// The rule fires only where both numbers exist and disagree; it never judges a size on its own.
/// That is deliberate: "the response is small" is not evidence -- a legitimately tiny file is
/// tiny, and [`SIZE_TOLERANCE_FLOOR_BYTES`] keeps it out of reach of this rule entirely. What is
/// evidence is a hoster saying 405 MB and then offering 1150 bytes, which is the shape of a
/// refusal served in place of the payload (RD-109-36).
#[must_use]
pub fn contradicts_announced_size(announced: u64, offered: u64) -> bool {
    let tolerance = (announced / 100 * SIZE_TOLERANCE_PERCENT).max(SIZE_TOLERANCE_FLOOR_BYTES);
    announced.abs_diff(offered) > tolerance
}

/// Server metadata used to select safe resume behavior.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub final_url: Url,
    pub total_bytes: Option<u64>,
    pub accepts_ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_disposition: Option<String>,
    pub content_type: Option<String>,
}

impl ProbeResult {
    /// Whether the response headers describe file content rather than a hoster page.
    ///
    /// Mirrors JDownloader's `looksLikeDownloadableContent`: an explicit attachment always
    /// counts, markup never does, and other text-ish types are only rejected while the
    /// payload is small enough to be an error page.
    #[must_use]
    pub fn looks_downloadable(&self) -> bool {
        looks_downloadable(
            self.content_disposition.as_deref(),
            self.content_type.as_deref(),
            self.total_bytes,
        )
    }
}

/// The same judgement on the bare headers of any response.
///
/// Shared with the transfer engine: a hoster that answers a chunk request with a throttle
/// notice sends exactly the response the probe would have rejected, and there must not be a
/// second, differently-minded copy of this rule.
#[must_use]
pub(crate) fn looks_downloadable(
    content_disposition: Option<&str>,
    content_type: Option<&str>,
    total_bytes: Option<u64>,
) -> bool {
    if content_disposition.is_some_and(is_attachment) {
        return true;
    }
    let Some(essence) = content_type.map(mime_essence) else {
        return true;
    };
    if is_markup(&essence) {
        return false;
    }
    if !is_texty(&essence) {
        return true;
    }
    total_bytes.is_some_and(|total| total >= LARGE_PAYLOAD_BYTES)
}

/// Probes with HEAD and falls back to a one-byte range request when metadata is incomplete.
pub async fn probe(client: &Client, url: Url) -> Result<ProbeResult, HttpDownloadError> {
    probe_with_headers(client, url, &[]).await
}

/// Probes while replaying the headers a resolver attached to the transfer (Referer, …).
pub async fn probe_with_headers(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
) -> Result<ProbeResult, HttpDownloadError> {
    let head = apply_headers(client.head(url.clone()), headers)
        .send()
        .await;
    // `Response::content_length()` reflects the body size hint, which is always zero for
    // HEAD responses; the advertised entity length lives in the Content-Length header.
    if let Ok(response) = head
        && response.status().is_success()
        && let Some(total) = header_content_length(response.headers())
    {
        return Ok(from_response(&response, Some(total), false));
    }

    let response = apply_headers(client.get(url), headers)
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(network_failure)?;
    if !response.status().is_success() {
        return Err(status_failure(response.status(), response.headers()));
    }
    let ranged = response.status() == StatusCode::PARTIAL_CONTENT;
    let total = if ranged {
        response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_total)
    } else {
        header_content_length(response.headers())
    };
    Ok(from_response(&response, total, ranged))
}

/// Reads the beginning of a non-file response so the hoster's own wording ("you have to
/// wait 30 minutes") reaches the user instead of a bare content type.
pub async fn peek_body_text(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    limit: usize,
) -> Option<String> {
    let response = apply_headers(client.get(url), headers).send().await.ok()?;
    let mut collected = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.ok()?);
        if collected.len() >= limit {
            collected.truncate(limit);
            break;
        }
    }
    let text = String::from_utf8_lossy(&collected);
    let condensed = strip_markup(&text);
    (!condensed.is_empty()).then_some(condensed)
}

/// Reads the beginning of a response **verbatim**, with its line structure intact.
///
/// [`peek_body_text`] exists to turn a hoster's HTML error page into one readable line, and
/// it strips tags and collapses whitespace to do that. A manifest is the opposite kind of
/// document: every newline is load-bearing and an MPD is nothing but tags. This returns the
/// bytes as they arrived, along with the content type and the URL the response actually came
/// from, which is what relative URIs inside the document have to resolve against.
pub async fn fetch_text_verbatim(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    limit: usize,
) -> Option<VerbatimBody> {
    let response = apply_headers(client.get(url), headers).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let final_url = response.url().clone();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut collected = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.ok()?);
        if collected.len() >= limit {
            collected.truncate(limit);
            break;
        }
    }
    Some(VerbatimBody {
        text: String::from_utf8_lossy(&collected).into_owned(),
        content_type,
        final_url,
    })
}

/// A body read verbatim, with the two headers a document needs to interpret itself.
#[derive(Clone, Debug)]
pub struct VerbatimBody {
    pub text: String,
    pub content_type: Option<String>,
    /// Where the response came from after redirects; relative URIs resolve against this.
    pub final_url: Url,
}

/// Downloads a bounded response body as bytes, following redirects.
///
/// For documents that are fetched to be *parsed* rather than stored — an NZB from an
/// indexer, say. Bytes rather than text because the caller hashes them, and a lossy UTF-8
/// conversion would change the hash.
pub async fn fetch_bytes(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    limit: usize,
) -> Result<Vec<u8>, anyhow::Error> {
    Ok(fetch_document(client, url, headers, limit).await?.bytes)
}

/// The same fetch, with the response headers the caller needs to interpret the document.
///
/// An indexer says what it just handed out — `Content-Disposition`, and the `X-DNZB-*`
/// headers SABnzbd established — and that is the only place the release name and a refusal
/// dressed up as `200 OK` can be read. Dropping the headers means naming the job after the
/// API endpoint every hit shares.
pub async fn fetch_document(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    limit: usize,
) -> Result<FetchedDocument, anyhow::Error> {
    let response = apply_headers(client.get(url), headers).send().await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {status}");
    }
    let headers = response.headers().clone();
    let mut collected = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk?);
        if collected.len() > limit {
            anyhow::bail!("response exceeds the {limit} byte limit");
        }
    }
    Ok(FetchedDocument {
        bytes: collected,
        headers,
    })
}

/// A document fetched to be parsed, with the response headers that describe it.
#[derive(Clone, Debug)]
pub struct FetchedDocument {
    pub bytes: Vec<u8>,
    pub headers: header::HeaderMap,
}

impl FetchedDocument {
    /// The header's value as a string, for the callers that read one by name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// A conditional GET whose `304` is an outcome rather than an error.
///
/// Feeds are polled on a schedule and mostly do not change, so `If-None-Match` /
/// `If-Modified-Since` are what keep an hourly subscription from downloading the same
/// document 24 times a day (RD-080-10).
pub async fn fetch_conditional(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    limit: usize,
) -> Result<ConditionalBody, anyhow::Error> {
    let response = apply_headers(client.get(url), headers).send().await?;
    let status = response.status();
    let final_url = response.url().clone();
    let etag = header_string(response.headers(), header::ETAG);
    let last_modified = header_string(response.headers(), header::LAST_MODIFIED);
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(ConditionalBody {
            body: None,
            etag,
            last_modified,
            final_url,
        });
    }
    if !status.is_success() {
        anyhow::bail!("HTTP {status}");
    }
    let mut collected = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk?);
        if collected.len() >= limit {
            collected.truncate(limit);
            break;
        }
    }
    Ok(ConditionalBody {
        body: Some(String::from_utf8_lossy(&collected).into_owned()),
        etag,
        last_modified,
        final_url,
    })
}

/// The result of [`fetch_conditional`]; `body` is `None` for a `304`.
#[derive(Clone, Debug)]
pub struct ConditionalBody {
    pub body: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub final_url: Url,
}

/// Collapses an HTML error page into a single line of readable text.
fn strip_markup(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut depth = 0_usize;
    for character in text.chars() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(character),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn apply_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        if let Ok(name) = header::HeaderName::from_bytes(name.as_bytes())
            && let Ok(value) = header::HeaderValue::from_str(value)
        {
            builder = builder.header(name, value);
        }
    }
    builder
}

fn mime_essence(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_attachment(disposition: &str) -> bool {
    disposition.to_ascii_lowercase().contains("attachment")
}

fn is_markup(essence: &str) -> bool {
    matches!(essence, "text/html" | "application/xhtml+xml")
}

fn is_texty(essence: &str) -> bool {
    matches!(
        essence,
        "text/plain"
            | "text/xml"
            | "text/css"
            | "text/javascript"
            | "application/xml"
            | "application/json"
            | "application/javascript"
    )
}

fn header_content_length(headers: &header::HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
}

fn from_response(response: &reqwest::Response, total: Option<u64>, ranged: bool) -> ProbeResult {
    let headers = response.headers();
    let accepts_ranges = ranged
        || headers
            .get(header::ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
    ProbeResult {
        final_url: response.url().clone(),
        total_bytes: total,
        accepts_ranges,
        etag: header_string(headers, header::ETAG).filter(|etag| !is_weak_etag(etag)),
        last_modified: header_string(headers, header::LAST_MODIFIED),
        content_disposition: header_string(headers, header::CONTENT_DISPOSITION),
        content_type: header_string(headers, header::CONTENT_TYPE),
    }
}

/// Whether an `ETag` is a weak validator (`W/"..."`).
///
/// A weak tag only promises semantic equivalence, so the body may legitimately differ
/// while the tag stays the same. Using it to decide that a partial file is still valid
/// would resume into different bytes, so it is treated as no validator at all and the
/// resume check falls back to size and `Last-Modified`. Some WebDAV servers emit only weak
/// tags, which is what surfaced this.
fn is_weak_etag(etag: &str) -> bool {
    let trimmed = etag.trim_start();
    trimmed.starts_with("W/") || trimmed.starts_with("w/")
}

fn header_string(headers: &header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.parse().ok()
}

/// First byte a `Content-Range` describes, as in `bytes 1024-2047/8192`.
///
/// The header, not the status, says which part of the entity a body carries: a server may
/// answer `200` and still describe the range it is sending, and only this tells that apart
/// from a full body that happens to arrive with the same status.
pub(crate) fn parse_content_range_start(value: &str) -> Option<u64> {
    let (unit, ranges) = value.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    ranges.trim().split_once('-')?.0.trim().parse().ok()
}

#[cfg(test)]
mod tests {

    /// RD-109-36: the rule exists only where two numbers disagree. The case that started it is
    /// a 405 MB release answered with a 1150-byte refusal page.
    #[test]
    fn an_offer_far_under_the_announcement_is_a_contradiction() {
        assert!(super::contradicts_announced_size(425_134_653, 1150));
        assert!(super::contradicts_announced_size(425_134_653, 0));
    }

    /// A hoster prints a rounded size; the exact length may differ by well under a percent.
    #[test]
    fn rounding_in_the_announcement_is_tolerated() {
        assert!(!super::contradicts_announced_size(425_134_653, 425_132_032));
        assert!(!super::contradicts_announced_size(
            425_134_653,
            425_134_653 + 4_000_000
        ));
    }

    /// The guard must never fire on a legitimately small file. Below the floor the two numbers
    /// are treated as agreeing whatever they are -- "it is small" is not evidence of anything.
    #[test]
    fn a_small_file_can_never_trip_the_rule() {
        assert!(!super::contradicts_announced_size(1, 1));
        assert!(!super::contradicts_announced_size(1150, 0));
        assert!(!super::contradicts_announced_size(500, 4_000));
        // Still caught once the announcement is large enough to mean something.
        assert!(super::contradicts_announced_size(1_000_000, 1150));
    }

    /// An oversized offer is as wrong as an undersized one: it is not the announced file either.
    #[test]
    fn an_offer_far_over_the_announcement_is_a_contradiction() {
        assert!(super::contradicts_announced_size(1_000_000, 900_000_000));
    }
    use axum::{Router, routing::get};

    use super::{
        ProbeResult, is_weak_etag, parse_content_range_start, parse_content_range_total, probe,
        strip_markup,
    };

    #[test]
    fn a_weak_etag_is_not_treated_as_a_validator() {
        // The bug this guards: a weak tag stays the same while the body legitimately
        // changes, so resuming on it continues into different bytes.
        assert!(is_weak_etag("W/\"abc\""));
        assert!(is_weak_etag("w/\"abc\""));
        assert!(!is_weak_etag("\"abc\""));
        assert!(!is_weak_etag("abc"));
    }

    fn sniffed(content_type: Option<&str>, disposition: Option<&str>, total: Option<u64>) -> bool {
        ProbeResult {
            final_url: "https://example.test/file".parse().expect("URL"),
            total_bytes: total,
            accepts_ranges: true,
            etag: None,
            last_modified: None,
            content_disposition: disposition.map(str::to_owned),
            content_type: content_type.map(str::to_owned),
        }
        .looks_downloadable()
    }

    #[test]
    fn parses_content_range() {
        assert_eq!(parse_content_range_total("bytes 0-0/4096"), Some(4096));
        assert_eq!(parse_content_range_total("bytes */*"), None);
    }

    #[test]
    fn parses_the_first_byte_a_content_range_describes() {
        assert_eq!(parse_content_range_start("bytes 0-0/4096"), Some(0));
        assert_eq!(
            parse_content_range_start("bytes 1024-2047/8192"),
            Some(1024)
        );
        assert_eq!(parse_content_range_start("Bytes 512-/8192"), Some(512));
        // An unsatisfied range names no first byte, and a unit that is not bytes says
        // nothing about offsets at all.
        assert_eq!(parse_content_range_start("bytes */8192"), None);
        assert_eq!(parse_content_range_start("items 0-9/50"), None);
        assert_eq!(parse_content_range_start("nonsense"), None);
    }

    #[test]
    fn a_hoster_landing_page_is_not_downloadable_content() {
        assert!(!sniffed(
            Some("text/html; charset=utf-8"),
            None,
            Some(30_000)
        ));
        // Markup stays rejected regardless of size: a big page is still a page.
        assert!(!sniffed(Some("text/html"), None, Some(8 * 1024 * 1024)));
        assert!(!sniffed(Some("application/json"), None, Some(512)));
    }

    #[test]
    fn real_payloads_and_attachments_stay_downloadable() {
        assert!(sniffed(Some("application/octet-stream"), None, Some(4096)));
        assert!(sniffed(Some("video/mp4"), None, None));
        assert!(sniffed(None, None, None));
        // An explicit attachment wins even when the type looks like markup.
        assert!(sniffed(
            Some("text/html"),
            Some("attachment; filename=\"archive.rar\""),
            Some(10)
        ));
        // A large text payload is a file, not an error page.
        assert!(sniffed(Some("text/plain"), None, Some(4 * 1024 * 1024)));
    }

    #[test]
    fn markup_is_condensed_into_a_readable_message() {
        let page = "<html><body><h1>Wait</h1>\n<p>You have to wait 30 minutes</p></body></html>";
        assert_eq!(strip_markup(page), "Wait You have to wait 30 minutes");
    }

    #[tokio::test]
    async fn head_probe_reports_the_advertised_content_length() {
        const PAYLOAD: &[u8] = b"cdn-payload-with-a-real-length";
        let app = Router::new().route("/file.bin", get(|| async { PAYLOAD }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });

        let url = format!("http://{address}/file.bin").parse().expect("URL");
        let result = probe(&reqwest::Client::new(), url).await.expect("probe");
        assert_eq!(result.total_bytes, Some(PAYLOAD.len() as u64));
    }
}
