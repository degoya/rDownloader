//! Reading the file page: the direct link, and every state the page can be in instead.
//!
//! The markers are the ones JDownloader's `MediafireCom` and pyLoad's `MediafireCom.py` read
//! (behaviour, not code — see the job file, section 4), and the live page of 2026-09-21
//! confirms the first: the anchor `id="downloadButton"` carries the direct link (the name and
//! size come from the API, which the resolver asks first). The other states — `form_captcha`, `form_password`,
//! `MalwareAdvisory`, `limitReachedTTL` — were not seen live and are covered by synthetic
//! fixtures that say so. Everything is a substring scan over one page; there is no HTML
//! parser in the sandbox and none is needed for four attributes.

use plugin_common::HttpResponse;

/// The kinds of captcha form the page can carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CaptchaKind {
    /// A reCAPTCHA v2 widget with its site key; answered through the host.
    Recaptcha { site_key: String },
    /// MediaFire's own checkbox, answered by posting `mf_captcha_response=1`.
    Checkbox,
    /// A form this plugin does not know how to answer.
    Unknown,
}

/// The `form_captcha` form: what it asks and what it already carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CaptchaForm {
    pub(crate) kind: CaptchaKind,
    /// Hidden fields, to be posted back with the answer.
    pub(crate) fields: Vec<(String, String)>,
}

/// Whether the answer is a page rather than a file.
#[must_use]
pub(crate) fn is_html(response: &HttpResponse) -> bool {
    match response.header("content-type") {
        Some(content_type) => content_type.to_ascii_lowercase().contains("text/html"),
        None => response.body.trim_ascii_start().starts_with(b"<"),
    }
}

/// The direct link the page offers, validated to point at a delivery host.
///
/// The download button first; then the `kNO` variable JD reads on older layouts; then any
/// `download<n>.mediafire.com` address written anywhere on the page.
#[must_use]
pub(crate) fn direct_link(html: &str) -> Option<String> {
    tags(html, "<a")
        .find(|tag| attribute(tag, "id").is_some_and(|id| id == "downloadButton"))
        .and_then(|tag| attribute(&tag, "href"))
        .and_then(|href| delivery_url(&unescape(&href)))
        .or_else(|| {
            after(html, "kNO = \"")
                .and_then(|rest| rest.split('"').next())
                .and_then(delivery_url)
        })
        .or_else(|| {
            html.match_indices("https://download")
                .map(|(start, _)| {
                    let rest = &html[start..];
                    let end = rest
                        .find(['"', '\'', ' ', '<', '\n', '\r', '\t'])
                        .unwrap_or(rest.len());
                    &rest[..end]
                })
                .find_map(delivery_url)
        })
}

/// Whether the page carries the malware advisory, which this plugin never clicks through.
#[must_use]
pub(crate) fn has_malware_advisory(html: &str) -> bool {
    html.contains("MalwareAdvisory")
}

/// `var limitReachedTTL = N;` — the per-IP threshold, in seconds. `Some(0)` when the page
/// says the threshold was exceeded without naming a time.
#[must_use]
pub(crate) fn threshold_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "limitReachedTTL";
    let mut seen = false;
    for (start, _) in html.match_indices(MARKER) {
        seen = true;
        let rest = html[start + MARKER.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(seconds) = digits.parse() {
            return Some(seconds);
        }
    }
    (seen || html.contains("Download Threshold Exceeded")).then_some(0)
}

/// Whether the page asks for the file's password.
#[must_use]
pub(crate) fn has_password_form(html: &str) -> bool {
    tags(html, "<form").any(|tag| names(&tag, "form_password"))
}

/// A short wait the site asks for: "Temporarily Unavailable", or "retry your download again
/// in N seconds".
#[must_use]
pub(crate) fn retry_seconds(html: &str) -> Option<u64> {
    if let Some(rest) = after(html, "retry your download again in ") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(seconds) = digits.parse::<u64>() {
            return Some(seconds.max(1));
        }
    }
    html.contains("Temporarily Unavailable").then_some(300)
}

/// The `form_captcha` form, when the page carries one.
#[must_use]
pub(crate) fn captcha_form(html: &str) -> Option<CaptchaForm> {
    let (start, _) = html
        .match_indices("<form")
        .find(|(start, _)| tag_at(html, *start).is_some_and(|tag| names(tag, "form_captcha")))?;
    let block = &html[start..];
    let block = &block[..block.find("</form>").unwrap_or(block.len())];
    let kind = match widget_site_key(block) {
        Some(site_key) => CaptchaKind::Recaptcha { site_key },
        None if block.contains("mf_captcha_response") => CaptchaKind::Checkbox,
        None => CaptchaKind::Unknown,
    };
    let fields = tags(block, "<input")
        .filter(|tag| attribute(tag, "type").is_none_or(|kind| kind.eq_ignore_ascii_case("hidden")))
        .filter_map(|tag| Some((attribute(&tag, "name")?, attribute(&tag, "value")?)))
        .map(|(name, value)| (unescape(&name), unescape(&value)))
        .collect();
    Some(CaptchaForm { kind, fields })
}

/// Explains why a page came back instead of a link, for the failure message: its title.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    let title = after(html, "<title>")
        .and_then(|rest| rest.split("</title>").next())
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty());
    match title {
        Some(title) => format!(
            "page titled \"{}\"",
            title.chars().take(80).collect::<String>()
        ),
        None => "page without a title".to_owned(),
    }
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    let mut body = String::new();
    for (name, value) in fields {
        if !body.is_empty() {
            body.push('&');
        }
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body.into_bytes()
}

/// The site key of a reCAPTCHA v2 widget inside `block`.
fn widget_site_key(block: &str) -> Option<String> {
    tags(block, "<div")
        .chain(tags(block, "<button"))
        .find(|tag| {
            attribute(tag, "class")
                .is_some_and(|class| class.split_whitespace().any(|item| item == "g-recaptcha"))
        })
        .and_then(|tag| attribute(&tag, "data-sitekey"))
        .filter(|key| !key.is_empty() && key.len() <= 256)
}

/// A page address that points at a delivery host, or nothing.
fn delivery_url(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    let absolute = match candidate.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => candidate.to_owned(),
    };
    let parsed = url::Url::parse(&absolute).ok()?;
    (parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(mediafire_common::address::is_download_host))
    .then(|| parsed.to_string())
}

/// The text after the first occurrence of `marker`.
fn after<'a>(html: &'a str, marker: &str) -> Option<&'a str> {
    html.find(marker).map(|start| &html[start + marker.len()..])
}

/// Every tag opening with `opening` (such as `<a`), as its text up to the closing `>`.
fn tags<'a>(html: &'a str, opening: &'a str) -> impl Iterator<Item = String> + 'a {
    html.match_indices(opening)
        .filter_map(|(start, _)| tag_at(html, start))
        .map(str::to_owned)
}

/// The tag whose `<` is at `start`, when what follows the opening is a tag boundary.
fn tag_at(html: &str, start: usize) -> Option<&str> {
    let rest = &html[start..];
    let name_end = rest[1..]
        .find(|character: char| !character.is_ascii_alphanumeric())
        .map_or(rest.len(), |end| end + 1);
    if !rest[name_end..].starts_with(|character: char| {
        character.is_whitespace() || character == '>' || character == '/'
    }) {
        return None;
    }
    let end = rest.find('>')?;
    Some(&rest[..=end])
}

/// Whether a tag's `name` or `id` is `expected`.
fn names(tag: &str, expected: &str) -> bool {
    attribute(tag, "name").is_some_and(|name| name == expected)
        || attribute(tag, "id").is_some_and(|id| id == expected)
}

/// One attribute of a tag, quoted either way, name matched without regard to case.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let start = from + found;
        let before = lower[..start].chars().next_back();
        let rest = tag[start + name.len()..].trim_start();
        if before.is_some_and(char::is_whitespace)
            && let Some(value) = rest.strip_prefix('=')
        {
            let value = value.trim_start();
            let quote = value.chars().next()?;
            return if quote == '"' || quote == '\'' {
                value[1..].split(quote).next().map(str::to_owned)
            } else {
                Some(
                    value
                        .split(|character: char| character.is_whitespace() || character == '>')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                )
            };
        }
        from = start + name.len();
    }
    None
}

/// The five entities HTML attributes and text commonly carry.
fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn form_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod tests;
