//! Target-independent parsing for Nitroflare's account-less (free) *website* flow, the
//! counterpart of [`crate::api`]'s JSON-API helpers. Shared verbatim by the native
//! (`native/free.rs`) and WebAssembly (`guest/free.rs`) adapters so both report byte-identical
//! failures; only `url` is used here, so it compiles on every target and every function is
//! unit-testable without a host (see `page/tests.rs`).
//!
//! IMPL-VERIFY (against JD's `NitroFlareCom.java`, `svn_trunk/src/jd/plugins/hoster/`, fetched
//! 2026-09-02 — the file is 1251 lines; `handleFreeDownload`'s website branch is lines 484-617
//! and `handleErrors` lines 687-732):
//! - **Verified verbatim**: the file page URL `https://<domain>/view/<fid>`
//!   (`getCorrectedDownloadURL`, line 214); the ajax headers `Accept: */*` and
//!   `X-Requested-With: XMLHttpRequest` (`setAjaxHeaders`, line 734); the timer request
//!   `POST /ajax/freeDownload.php` with `method=startTimer&fileId=<id>` and its answer having to
//!   be exactly `1` (lines 518-523); the pre-download wait coming from the **file page**'s
//!   `<div id="CountDownTimer" data-timer="(\d+)"` (line 525) with JD's own 60-second fallback
//!   when it is absent (line 550); the second request carrying `method=fetchDownload` plus the
//!   token in both `captcha` and `g-recaptcha-response` (lines 530, 583-584); the rejected-captcha
//!   phrases "The captcha wasn't entered correctly" / "You have to fill the captcha" (line 599);
//!   the direct-link patterns — a quoted `https://<sub>.<hoster domain>/...` URL and the
//!   `href="..."`-before-"Click here to download" fallback (lines 608-611); and the limit
//!   phrasings in [`ip_block_seconds`] / [`is_premium_only`] (lines 699-722).
//! - **Deliberate deviation from the brief**: the brief said the wait seconds are parsed from the
//!   *startTimer answer*. JD reads them from the file page instead, which is what
//!   [`countdown_seconds`] does; [`timer_start`] still accepts a numeric answer as a
//!   [`TimerStart::Countdown`] so a drifted live site that does state them there keeps working.
//! - **Deliberate deviation from JD**: JD's website branch additionally posts `/ajax/setCookie.php`
//!   and `/ajax/randHash.php` and re-posts `goToFreePage=` in a randomised loop (lines 497-517).
//!   Those exist to mimic a browser's timing; they establish no state this flow needs (cookies are
//!   kept by the pooled anonymous client), and each one is another chance to be fingerprinted, so
//!   they are not reproduced.
//! - **Deliberate deviation from JD**: JD retries a rejected captcha up to five times. A rejection
//!   is reported here instead, so a second paid captcha is never spent inside one resolve; the
//!   scheduler retries the whole flow, which also gets a fresh countdown.

/// Which answer `POST /ajax/freeDownload.php method=startTimer` gave.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimerStart {
    /// JD's expected answer: the body is exactly `1`.
    Started,
    /// The body is a bare number of seconds — not what JD sees, but accepted as a started
    /// countdown that also states its own duration (see the module doc).
    Countdown(u64),
    /// Anything else; the caller reports it with the body as the failure's parameter.
    Unrecognized,
}

/// Classifies the timer-start answer.
#[must_use]
pub(crate) fn timer_start(body: &str) -> TimerStart {
    let trimmed = body.trim();
    if trimmed == "1" {
        return TimerStart::Started;
    }
    match trimmed.parse::<u64>() {
        Ok(seconds) if seconds > 0 => TimerStart::Countdown(seconds),
        _ => TimerStart::Unrecognized,
    }
}

/// Seconds the file page's countdown states (`<div id="CountDownTimer" data-timer="N">`).
#[must_use]
pub(crate) fn countdown_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "CountDownTimer";
    const ATTRIBUTE: &str = "data-timer=";
    let at = html.find(MARKER)? + MARKER.len();
    let window = clamp(&html[at..], 200);
    let offset = window.find(ATTRIBUTE)? + ATTRIBUTE.len();
    digits_at(quoted_value(window[offset..].trim_start())?.as_str())
}

/// JD's fallback when the page states no countdown (`long waitMillis = 60;` then the "Failed to
/// parse pre-download-waittime from html" warning, `NitroFlareCom.java:550-555`).
pub(crate) const DEFAULT_WAIT_SECONDS: u64 = 60;

/// The reCAPTCHA v2 site key the file page embeds: the widget's `data-sitekey` attribute first,
/// then a key carried in a `/recaptcha/` script or iframe URL.
#[must_use]
pub(crate) fn recaptcha_site_key(html: &str) -> Option<String> {
    const ATTRIBUTE: &str = "data-sitekey=";
    if let Some(at) = html.find(ATTRIBUTE)
        && let Some(key) =
            quoted_value(html[at + ATTRIBUTE.len()..].trim_start()).filter(|key| !key.is_empty())
    {
        return Some(key);
    }
    site_key_from_recaptcha_url(html)
}

/// `.../recaptcha/api.js?render=<key>` or `.../recaptcha/api2/anchor?...&k=<key>`. The length
/// floor rejects `render=explicit`, which is a rendering mode rather than a key.
fn site_key_from_recaptcha_url(html: &str) -> Option<String> {
    let at = html.find("/recaptcha/")?;
    let rest = clamp(&html[at..], 400);
    ["render=", "k="].into_iter().find_map(|marker| {
        let offset = rest.find(marker)?;
        let value: String = rest[offset + marker.len()..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
            })
            .collect();
        (value.len() >= 20).then_some(value)
    })
}

/// Seconds this IP must wait before Nitroflare grants another free download, or `Some(0)` when
/// the page states a limit without naming a duration. `None` means no limit notice at all.
///
/// Mirrors the free-mode branches of JD's `handleErrors`. JD raises
/// `ERROR_HOSTER_TEMPORARILY_UNAVAILABLE` for the VPN/overload notices rather than
/// `ERROR_IP_BLOCKED`; they are reported as an IP block here on purpose, because they are just as
/// address-bound and the scheduler should hold back this hoster's other free links instead of
/// spending a wait and a paid captcha on each of them.
#[must_use]
pub(crate) fn ip_block_seconds(html: &str) -> Option<u64> {
    // "Free downloading is not possible. You have to wait 70 minutes to download your next file."
    if let Some(minutes) = number_between(html, "You have to wait", "minutes to download") {
        return Some(minutes.saturating_mul(60));
    }
    // JD: 30 minutes.
    if html.contains("Your ip has been blocked") || html.contains("Your ip is been blocked") {
        return Some(30 * 60);
    }
    // JD: one hour for both VPN/proxy notices.
    if html.contains("You can't use free download with a VPN")
        || html.contains("You can`t use free download with a VPN")
        || html.contains("To continue this download please")
    {
        return Some(60 * 60);
    }
    // JD: five minutes.
    if html.contains("Free download is currently unavailable due to overloading in the server") {
        return Some(5 * 60);
    }
    // A limit whose duration the page did not state; the hold-off is the scheduler's own.
    html.contains("Free downloading is not possible")
        .then_some(0)
}

/// Whether the page says the file is premium-only (JD's `handleErrors`, line 704).
#[must_use]
pub(crate) fn is_premium_only(html: &str) -> bool {
    html.contains("This file is available with premium key only")
        || html.contains("This file is available with Premium only")
}

/// Whether the answer rejected the captcha (JD's `handleFreeDownload`, line 599).
#[must_use]
pub(crate) fn is_wrong_captcha(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("the captcha wasn't entered correctly")
        || lower.contains("the captcha wasn`t entered correctly")
        || lower.contains("you have to fill the captcha")
}

/// The direct link the `fetchDownload` answer carries: a quoted absolute URL on one of the
/// hoster's own subdomains first, then JD's "Click here to download" anchor fallback.
#[must_use]
pub(crate) fn direct_link(html: &str) -> Option<String> {
    quoted_provider_link(html).or_else(|| click_here_link(html))
}

fn quoted_provider_link(html: &str) -> Option<String> {
    let mut cursor = html;
    while let Some(at) = cursor.find("http") {
        let rest = &cursor[at..];
        let end = rest
            .find(|character: char| {
                matches!(character, '\'' | '"' | '<' | '>' | '\\') || character.is_whitespace()
            })
            .unwrap_or(rest.len());
        let candidate = decode_entities(&rest[..end]);
        if is_download_link(&candidate) {
            return Some(candidate);
        }
        cursor = &rest["http".len()..];
    }
    None
}

/// A link on one of the hoster's own hosts that is not the file page itself.
fn is_download_link(candidate: &str) -> bool {
    let Ok(url) = url::Url::parse(candidate) else {
        return false;
    };
    url.host_str().is_some_and(is_provider_host)
        && !url.path().starts_with("/view/")
        && !url.path().starts_with("/watch/")
        && url.path().len() > 1
}

fn click_here_link(html: &str) -> Option<String> {
    const MARKER: &str = "Click here to download";
    let at = html.find(MARKER)?;
    let anchor = html[..at].rfind("href=")? + "href=".len();
    quoted_value(html[anchor..].trim_start()).filter(|link| link.starts_with("http"))
}

/// Whether `host` belongs to the hoster, so a parsed link is followed only when it does. JD's
/// plugin also serves the `nitroflare.net`/`nitro.download` aliases; this plugin's manifest pins
/// `nitroflare.com`, so a link anywhere else is refused rather than fetched.
#[must_use]
pub(crate) fn is_provider_host(host: &str) -> bool {
    const PRIMARY: &str = "nitroflare.com";
    host == PRIMARY || host.ends_with(&format!(".{PRIMARY}"))
}

/// The `filename=` parameter of a `Content-Disposition` header value.
#[must_use]
pub(crate) fn file_name_from_disposition(value: &str) -> Option<String> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("filename")
            .then(|| value.trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// The file name a Nitroflare direct link ends with, used when the transfer carries no
/// `Content-Disposition`.
#[must_use]
pub(crate) fn url_file_name(url: &url::Url) -> Option<String> {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty() && segment.contains('.'))
        .map(str::to_owned)
}

/// Explains why a page carried none of the markers the flow needs, for the failure message.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    for marker in ["class=\"error\"", "class=\"err\"", "class=\"alert-danger\""] {
        if let Some(text) = element_text(html, marker) {
            return format!("page message: {text}");
        }
    }
    match element_text(html, "<title>") {
        Some(title) => format!("page \"{title}\" carries no free-download markers"),
        None => "the response page carries no free-download markers".to_owned(),
    }
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(&str, &str)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish()
        .into_bytes()
}

// --- primitives ------------------------------------------------------------------------------

/// The first number appearing between `start` and `end`, within a short window so a number
/// further down the page cannot be mistaken for the one being looked for.
fn number_between(html: &str, start: &str, end: &str) -> Option<u64> {
    let after = &html[html.find(start)? + start.len()..];
    let window = clamp(after, 80);
    let limit = window.find(end)?;
    let digits_at_index = window[..limit].find(|character: char| character.is_ascii_digit())?;
    digits_at(&window[digits_at_index..])
}

fn digits_at(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The content of a quoted attribute at the front of `rest`; `Some("")` for an explicitly empty
/// value, `None` when `rest` does not start with a quote.
fn quoted_value(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[quote.len_utf8()..];
    let end = value.find(quote)?;
    Some(decode_entities(&value[..end]))
}

/// Text following `marker` up to the next tag, whitespace collapsed and capped at 160 bytes.
fn element_text(html: &str, marker: &str) -> Option<String> {
    let at = html.find(marker)? + marker.len();
    let rest = &html[at..];
    let rest = if marker.starts_with("class=") {
        &rest[rest.find('>')? + 1..]
    } else {
        rest
    };
    let end = rest.find('<').unwrap_or(rest.len());
    let collapsed = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
    let text = clamp(&collapsed, 160).to_owned();
    (!text.is_empty()).then_some(text)
}

/// The named HTML entities these pages use; anything else is left as it stands.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `text` truncated to at most `max` bytes, never splitting a UTF-8 character.
fn clamp(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
#[path = "page/tests.rs"]
mod tests;
