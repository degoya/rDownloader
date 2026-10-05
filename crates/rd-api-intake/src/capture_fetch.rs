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
//! - **Not this machine.** The address, and every address a redirect leads to, is held to the
//!   address guard the LinkGrabber's proposed links keep to (`rd_http::AddressPolicy`): the
//!   person's own network is fine — an indexer on the NAS is a real setup — but loopback, the
//!   service's own addresses and the link-local block are not (security review 2026-09-28,
//!   finding 9). A capture token would otherwise be a way to make the service request its own
//!   API or a cloud metadata endpoint.

use std::{fmt, time::Duration};

use axum::http::{HeaderValue, header};
use rd_api_core::input_checks::{BodyError, read_bounded_body};
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
///
/// `guard` is the address policy every hop is held to; `None` only where
/// `AppState::with_local_capture_fetches` asked for it.
pub(crate) async fn fetch_once(
    plan: &FetchPlan,
    max_bytes: usize,
    guard: Option<&rd_http::AddressPolicy>,
) -> Result<Fetched, ApiError> {
    // No cookie store: a `Set-Cookie` in the answer is dropped with the client.
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT);
    if let Some(policy) = guard {
        // The name is resolved again to connect; the guard sees that answer too, so a name
        // that pointed elsewhere at the check below cannot point inside now.
        builder = builder.dns_resolver(rd_http::GuardedResolver::system(policy.clone()));
    }
    let client = builder.build().map_err(anyhow::Error::new)?;
    let mut current = plan.url.clone();
    for _ in 0..=MAX_REDIRECTS {
        // Every hop, not only the first: a public address that redirects to loopback is the
        // way around a check made once.
        if let Some(policy) = guard {
            screen_hop(policy, &current).await?;
        }
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
            if rd_http::is_refusal(&error) {
                return address_refused();
            }
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

async fn bounded_body(response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>, ApiError> {
    read_bounded_body(response, max_bytes)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge => file_too_large(max_bytes),
            BodyError::Interrupted(_) => fetch_failed().with_param("reason", "interrupted"),
        })
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

/// Refuses `target` when the policy does: its literal address, or any address its name
/// resolves to. A name without an address is left to fail as unreachable.
pub(crate) async fn screen_hop(
    policy: &rd_http::AddressPolicy,
    target: &Url,
) -> Result<(), ApiError> {
    match rd_http::check_target(policy, &rd_http::SystemLookup, target).await {
        Err(rd_http::TargetRefusal::Refused(_)) => Err(address_refused()),
        Ok(_) | Err(rd_http::TargetRefusal::Unresolved(_)) => Ok(()),
    }
}

fn address_refused() -> ApiError {
    ApiError::forbidden(
        "capture.fetch_address_refused",
        "The address points at this machine or at an address the service may not request",
    )
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
#[path = "capture_fetch_tests.rs"]
mod tests;
