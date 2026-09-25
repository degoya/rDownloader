//! One fetch of a browser download, with the browser's cookies for that one host (RD-130-16).
//!
//! The extension hands over an address and the cookies the browser would send there, because an
//! indexer's cart answers only a signed-in session. They arrive the way `capture/cookies` takes
//! a session — one string, a Netscape cookie file or a `Cookie` header, read by the one parser
//! the service has for either (`rd_core::parse_cookie_file`). Three rules hold here, and each is
//! what the person agreed to when they allowed the site in the extension:
//!
//! - **Once.** The address is fetched exactly one time, here, and the bytes go straight to the
//!   importer. No link check, no probe, no second request: an indexer counts every fetch against
//!   the account's download limit, and a one-time link is spent after the first.
//! - **That host only.** A cookie the browser would not send to the handed-over address — set
//!   for another domain, another path, or `Secure` on an `http` address — refuses the hand-over.
//!   The cookies then travel with a request whose origin — scheme, host and port — is the
//!   handed-over address's own. A redirect elsewhere is followed without them, and a downgrade
//!   from `https` to `http` on the same host counts as elsewhere.
//! - **Never kept, never written down.** The cookies live in this request's memory and nowhere
//!   else: no cookie store on the client, no row, no log line. The header is marked sensitive,
//!   which keeps it out of `Debug` output and out of HTTP/2 header compression tables, and
//!   [`CaptureCookie`]'s own `Debug` prints no value.

use std::{fmt, time::Duration};

use axum::http::{HeaderValue, header};
use url::Url;

use crate::ApiError;

/// The most cookies one hand-over may carry. A browser holds at most 180 per domain; a site
/// that sends more than this to one address is not a session worth taking.
pub(crate) const MAX_COOKIES: usize = 100;

/// The longest `Cookie` header this builds, in bytes. Servers commonly refuse beyond 8 KiB.
pub(crate) const MAX_COOKIE_HEADER_BYTES: usize = 16 * 1024;

/// The longest cookie string taken: a Netscape row carries domain, path, flags and expiry
/// besides the pair, so the file is several times the header it becomes.
pub(crate) const MAX_COOKIE_TEXT_BYTES: usize = 64 * 1024;

const MAX_COOKIE_NAME_BYTES: usize = 256;
const MAX_COOKIE_VALUE_BYTES: usize = 4096;
const MAX_HEADER_VALUE_BYTES: usize = 2048;
const MAX_REDIRECTS: usize = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
const MIB: usize = 1024 * 1024;

/// One cookie as the browser would send it: a name and a value, and nothing about where it came
/// from — which host it goes to is this service's rule, not the caller's claim.
///
/// Not a request type of its own: the cookies arrive as one `write_only` string, as they do at
/// `capture/cookies`, because an array property cannot be `writeOnly` in the API document and a
/// cookie list must never read as something the API hands back.
#[derive(Clone)]
pub(crate) struct CaptureCookie {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for CaptureCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// A validated fetch: the address and the headers that go with it.
pub(crate) struct FetchPlan {
    url: Url,
    cookie: Option<HeaderValue>,
    referrer: Option<HeaderValue>,
    user_agent: HeaderValue,
}

/// What the one fetch brought back.
#[derive(Debug)]
pub(crate) struct Fetched {
    pub bytes: Vec<u8>,
    /// From the answer's `Content-Disposition`, else the last segment of the final address.
    pub file_name: Option<String>,
}

impl FetchPlan {
    /// Checks everything the caller sent before anything leaves this machine.
    pub(crate) fn new(
        url: &str,
        cookies: Option<&str>,
        referrer: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<Self, ApiError> {
        let mut url = http_url(url).ok_or_else(|| {
            ApiError::bad_request("capture.url_invalid", "The address must be http or https")
        })?;
        url.set_fragment(None);
        let referrer = match referrer.map(str::trim).filter(|text| !text.is_empty()) {
            None => None,
            Some(text) => Some(
                http_url(text)
                    .and_then(|referrer| header_value(referrer.as_str()))
                    .ok_or_else(|| header_invalid("referrer"))?,
            ),
        };
        let user_agent = match user_agent.map(str::trim).filter(|text| !text.is_empty()) {
            None => HeaderValue::from_static(concat!("rDownloader/", env!("CARGO_PKG_VERSION"))),
            Some(text) => header_value(text).ok_or_else(|| header_invalid("user_agent"))?,
        };
        let cookie = cookie_header(&cookies_for(cookies.unwrap_or_default(), &url)?)?;
        Ok(Self {
            url,
            cookie,
            referrer,
            user_agent,
        })
    }

    /// The address, for a log line that names the host and nothing else.
    pub(crate) fn host(&self) -> &str {
        self.url.host_str().unwrap_or_default()
    }
}

/// Whether the cookies go along to `hop`: only to the handed-over address's own origin.
pub(crate) fn cookies_travel(handed_over: &Url, hop: &Url) -> bool {
    handed_over.origin() == hop.origin()
}

/// The cookies in `text` — a Netscape cookie file or a `Cookie` header — that the browser would
/// send to `url`, as name and value; empty for an empty string.
///
/// A row the browser would not send there — another domain, a public suffix, another path, or
/// `Secure` on an `http` address — refuses the whole hand-over under
/// `capture.cookies_invalid` (`reason: outside`) rather than being dropped: the extension reads
/// exactly what the browser sends to that address, so anything else is not what the person
/// agreed to hand over. A header carries no domain, path or flag of its own and is bound to the
/// address's host by the parser.
pub(crate) fn cookies_for(text: &str, url: &Url) -> Result<Vec<CaptureCookie>, ApiError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    if text.len() > MAX_COOKIE_TEXT_BYTES {
        return Err(cookies_invalid("too_large").with_param("max_bytes", MAX_COOKIE_TEXT_BYTES));
    }
    let host = url.host_str().unwrap_or_default();
    // The rule `capture/cookies` and every cookie import ask (RD-120-49): the host itself or a
    // domain above it, never a public suffix.
    let scope = rd_http::CookieScope::new(url, false).map_err(|_| cookies_invalid("outside"))?;
    let rows = rd_core::parse_cookie_file(text, host).map_err(|_| cookies_invalid("format"))?;
    rows.into_iter()
        .map(|row| {
            let sent_there = scope.admit(&row.domain).is_ok()
                && row.matches_host(host)
                && path_matches(&row.path, url.path())
                && (!row.secure || url.scheme() == "https");
            if !sent_there {
                return Err(cookies_invalid("outside"));
            }
            Ok(CaptureCookie {
                name: row.name,
                value: row.value,
            })
        })
        .collect()
}

/// RFC 6265 5.1.4: the cookie's path is the request's, or a prefix of it that ends at a `/`.
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

/// The `Cookie` header for these cookies, marked sensitive; `None` for none.
///
/// Validated against RFC 6265's grammar on the side that matters: a name is a token and a value
/// carries no `;`, no control character and nothing outside printable ASCII, so nothing a
/// caller sends can end the header or start another one.
pub(crate) fn cookie_header(cookies: &[CaptureCookie]) -> Result<Option<HeaderValue>, ApiError> {
    if cookies.is_empty() {
        return Ok(None);
    }
    if cookies.len() > MAX_COOKIES {
        return Err(cookies_invalid("too_many").with_param("max_cookies", MAX_COOKIES));
    }
    let mut text = String::new();
    for cookie in cookies {
        if cookie.name.is_empty()
            || cookie.name.len() > MAX_COOKIE_NAME_BYTES
            || !cookie.name.bytes().all(is_token_byte)
        {
            return Err(cookies_invalid("name"));
        }
        if cookie.value.len() > MAX_COOKIE_VALUE_BYTES
            || !cookie
                .value
                .bytes()
                .all(|byte| (0x20..=0x7e).contains(&byte) && byte != b';')
        {
            return Err(cookies_invalid("value"));
        }
        if !text.is_empty() {
            text.push_str("; ");
        }
        text.push_str(&cookie.name);
        text.push('=');
        text.push_str(&cookie.value);
    }
    if text.len() > MAX_COOKIE_HEADER_BYTES {
        return Err(cookies_invalid("too_large").with_param("max_bytes", MAX_COOKIE_HEADER_BYTES));
    }
    let mut value = HeaderValue::from_str(&text).map_err(|_| cookies_invalid("value"))?;
    value.set_sensitive(true);
    Ok(Some(value))
}

/// Fetches the plan's address once, following redirects by hand so each hop is decided here.
pub(crate) async fn fetch_once(plan: &FetchPlan, max_bytes: usize) -> Result<Fetched, ApiError> {
    // No cookie store: a `Set-Cookie` in the answer is dropped with the client.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .build()
        .map_err(anyhow::Error::new)?;
    let mut current = plan.url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client
            .get(current.clone())
            .header(header::USER_AGENT, plan.user_agent.clone());
        if let Some(referrer) = &plan.referrer {
            request = request.header(header::REFERER, referrer.clone());
        }
        if let Some(cookie) = &plan.cookie
            && cookies_travel(&plan.url, &current)
        {
            request = request.header(header::COOKIE, cookie.clone());
        }
        // The error text of a failed send names the address, and the address may carry a
        // token; the host is all a log line gets.
        let response = request.send().await.map_err(|error| {
            tracing::info!(
                host = plan.host(),
                timeout = error.is_timeout(),
                "capture fetch failed"
            );
            fetch_failed().with_param("reason", "unreachable")
        })?;
        let status = response.status();
        if status.is_redirection() {
            current = response
                .headers()
                .get(header::LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| current.join(location).ok())
                .filter(|next| matches!(next.scheme(), "http" | "https"))
                .ok_or_else(|| fetch_failed().with_param("reason", "redirect_invalid"))?;
            continue;
        }
        if !status.is_success() {
            return Err(fetch_failed().with_param("status", status.as_u16()));
        }
        let file_name = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .and_then(crate::link_check_probe::disposition_file_name)
            .or_else(|| last_segment(&current));
        let bytes = bounded_body(response, max_bytes).await?;
        return Ok(Fetched { bytes, file_name });
    }
    Err(fetch_failed().with_param("reason", "too_many_redirects"))
}

async fn bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(file_too_large(max_bytes));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| fetch_failed().with_param("reason", "interrupted"))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > max_bytes {
            return Err(file_too_large(max_bytes));
        }
    }
    Ok(body)
}

fn http_url(text: &str) -> Option<Url> {
    Url::parse(text.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn header_value(text: &str) -> Option<HeaderValue> {
    if text.len() > MAX_HEADER_VALUE_BYTES
        || !text.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return None;
    }
    HeaderValue::from_str(text).ok()
}

fn last_segment(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.next_back()?;
    let decoded = percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .ok()?
        .trim()
        .to_owned();
    (!decoded.is_empty()).then_some(decoded)
}

/// RFC 7230's `tchar`: what a cookie name may consist of.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn cookies_invalid(reason: &str) -> ApiError {
    ApiError::bad_request(
        "capture.cookies_invalid",
        "A cookie cannot be sent as given",
    )
    .with_param("reason", reason)
}

fn header_invalid(field: &str) -> ApiError {
    ApiError::bad_request(
        "capture.header_invalid",
        "A request header cannot be sent as given",
    )
    .with_param("field", field)
}

fn fetch_failed() -> ApiError {
    ApiError::bad_gateway("capture.fetch_failed", "The address could not be fetched")
}

pub(crate) fn file_too_large(max_bytes: usize) -> ApiError {
    let max_mib = max_bytes / MIB;
    ApiError::payload_too_large(
        "capture.file_too_large",
        format!("The file exceeds the {max_mib} MiB a hand-over may carry"),
    )
    .with_param("max_mib", max_mib)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{
        CaptureCookie, FetchPlan, MAX_COOKIES, cookie_header, cookies_for, cookies_travel,
    };

    fn cookie(name: &str, value: &str) -> CaptureCookie {
        CaptureCookie {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn cookies_go_to_the_handed_over_origin_and_nowhere_else() {
        let handed = url("https://indexer.test/getnzb/abc");
        assert!(cookies_travel(
            &handed,
            &url("https://indexer.test/other?x=1")
        ));
        assert!(cookies_travel(&handed, &url("https://indexer.test:443/")));
        for elsewhere in [
            "https://www.indexer.test/getnzb/abc",
            "https://cdn.indexer.test/",
            "https://indexer.test.evil.test/",
            "https://indexer.test:8443/",
            "http://indexer.test/getnzb/abc",
            "https://other.test/",
        ] {
            assert!(!cookies_travel(&handed, &url(elsewhere)), "{elsewhere}");
        }
    }

    #[test]
    fn the_header_joins_the_cookies_and_is_marked_sensitive() {
        let header = cookie_header(&[cookie("uid", "7"), cookie("__Secure-sess", "a.b-c")])
            .expect("valid")
            .expect("some");
        assert_eq!(
            header.to_str().expect("ascii"),
            "uid=7; __Secure-sess=a.b-c"
        );
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains("a.b-c"));
        assert!(cookie_header(&[]).expect("none").is_none());
    }

    /// Nothing a caller sends may end the header or start another one.
    #[test]
    fn a_cookie_that_could_break_the_header_is_refused() {
        for bad in [
            cookie("", "v"),
            cookie("na me", "v"),
            cookie("name;", "v"),
            cookie("name", "v; other=1"),
            cookie("name", "v\r\nX-Injected: 1"),
            cookie("name", "caf\u{e9}"),
        ] {
            let error = cookie_header(&[bad]).expect_err("refused");
            assert_eq!(error.code(), "capture.cookies_invalid");
        }
        let many: Vec<CaptureCookie> = (0..=MAX_COOKIES)
            .map(|index| cookie(&format!("c{index}"), "v"))
            .collect();
        assert_eq!(
            cookie_header(&many).expect_err("refused").code(),
            "capture.cookies_invalid"
        );
    }

    #[test]
    fn a_cookie_never_shows_its_value_in_debug_output() {
        let text = format!("{:?}", cookie("session", "s3cr3t-value"));
        assert!(text.contains("session"));
        assert!(!text.contains("s3cr3t-value"));
    }

    #[test]
    fn the_plan_takes_only_web_addresses_and_clean_headers() {
        assert_eq!(
            FetchPlan::new("ftp://indexer.test/x", None, None, None)
                .err()
                .map(|error| error.code().to_owned()),
            Some("capture.url_invalid".to_owned())
        );
        assert_eq!(
            FetchPlan::new(
                "https://indexer.test/x",
                None,
                Some("javascript:alert(1)"),
                None
            )
            .err()
            .map(|error| error.code().to_owned()),
            Some("capture.header_invalid".to_owned())
        );
        assert_eq!(
            FetchPlan::new("https://indexer.test/x", None, None, Some("Agent\r\nX: 1"))
                .err()
                .map(|error| error.code().to_owned()),
            Some("capture.header_invalid".to_owned())
        );
        let plan = FetchPlan::new("https://indexer.test/x#part", Some(""), Some(""), Some(" "))
            .expect("plan");
        assert_eq!(plan.url.as_str(), "https://indexer.test/x");
        assert!(plan.referrer.is_none());
        assert!(
            plan.cookie.is_none(),
            "an empty cookie string is no cookies"
        );
        assert!(
            plan.user_agent
                .to_str()
                .expect("ascii")
                .starts_with("rDownloader/")
        );
    }

    /// Both spellings `capture/cookies` takes, and only what the browser would send there.
    #[test]
    fn the_cookies_are_the_ones_the_browser_would_send_to_that_address() {
        let cart = url("https://indexer.test/getnzb/abc");
        let file = "# Netscape HTTP Cookie File\n\
                    indexer.test\tFALSE\t/\tTRUE\t0\tuid\t7\n\
                    #HttpOnly_.indexer.test\tTRUE\t/getnzb\tTRUE\t1893456000\tsess\ta=b==\n";
        let cookies = cookies_for(file, &cart).expect("parsed");
        let pairs: Vec<(&str, &str)> = cookies
            .iter()
            .map(|cookie| (cookie.name.as_str(), cookie.value.as_str()))
            .collect();
        assert_eq!(pairs, [("uid", "7"), ("sess", "a=b==")]);
        let header = cookie_header(&cookies).expect("valid").expect("some");
        assert_eq!(header.to_str().expect("ascii"), "uid=7; sess=a=b==");
        let from_header = cookies_for("uid=7; sess=a=b==", &cart).expect("header");
        assert_eq!(from_header.len(), 2);
        assert!(cookies_for("  ", &cart).expect("none").is_empty());
        for outside in [
            "other.test\tFALSE\t/\tFALSE\t0\tuid\t7",
            "www.indexer.test\tFALSE\t/\tFALSE\t0\tuid\t7",
            ".test\tTRUE\t/\tFALSE\t0\tuid\t7",
            "indexer.test\tFALSE\t/account\tFALSE\t0\tuid\t7",
            "indexer.test\tFALSE\t/getnzbx\tFALSE\t0\tuid\t7",
        ] {
            let error = cookies_for(outside, &cart).expect_err(outside);
            assert_eq!(error.code(), "capture.cookies_invalid", "{outside}");
        }
        let plain = url("http://indexer.test/getnzb/abc");
        assert!(cookies_for("indexer.test\tFALSE\t/\tTRUE\t0\tuid\t7", &plain).is_err());
        assert!(super::path_matches("/getnzb", "/getnzb/abc"));
        assert!(super::path_matches("/", "/getnzb/abc"));
        assert!(!super::path_matches("/getnzb", "/getnzbx"));
    }

    /// Nothing that could end the header or start another one gets as far as the wire.
    #[test]
    fn a_cookie_string_that_could_break_the_header_is_refused() {
        let cart = url("https://indexer.test/getnzb/abc");
        for bad in ["novalue", "uid=7\r\nX-Injected: 1"] {
            let refused = cookies_for(bad, &cart).and_then(|cookies| cookie_header(&cookies));
            assert_eq!(
                refused.expect_err(bad).code(),
                "capture.cookies_invalid",
                "{bad}"
            );
        }
        let long = format!("a={}", "b".repeat(super::MAX_COOKIE_TEXT_BYTES));
        assert_eq!(
            cookies_for(&long, &cart).expect_err("long").code(),
            "capture.cookies_invalid"
        );
    }
}
