//! Target-independent parsing for Rapidgator's account-less (free) *website* flow, the
//! counterpart of [`crate::api`]'s JSON-API helpers. Shared verbatim by the native
//! (`native/free.rs`) and WebAssembly (`guest/free.rs`) adapters so both report byte-identical
//! failures; only `serde`/`serde_json`/`url` and `plugin_common::html` are used here, so it
//! compiles on every target and every function is unit-testable without a host (see
//! `page/tests.rs`).
//!
//! IMPL-VERIFY (the known free website flow, checked 2026-09-02):
//! - **Verified verbatim**: the three file-page markers `var startTimerUrl = '...';`, `var fid =
//!   <digits>;` and `var secs = <digits>;`; the timer-start request `startTimerUrl + "?fid=" + fid`
//!   with `X-Requested-With: XMLHttpRequest`; the `{"state":"started","sid":...}` answer;
//!   `/download/AjaxGetDownloadLink?sid=` answering `{"state":"done"}`; `GET /download/captcha` and
//!   the form found by `id="captchaform"`; the two token fields `DownloadCaptchaForm[verifyCode]` +
//!   `g-recaptcha-response`; going on without a captcha when no `captchaform` is present; the
//!   final-URL regex `'(https?://<host>//\?r=download/index&session_id=<alnum>)'` with the
//!   `location.href = '...'` fallback; the rejected-captcha phrases "Please fix the following input
//!   errors" / "The verification code is incorrect"; the known fixed reCAPTCHA v2 site key (see
//!   [`RECAPTCHA_SITE_KEY_FALLBACK`]); the limit phrasings in [`ip_block_seconds`]; and the content
//!   URL being `https://<main domain>/file/<fid>`, the main domain being `rapidgator.net`.
//! - **Assumed** (not verified, so kept deliberately defensive): that the file page itself carries
//!   a `data-sitekey`/reCAPTCHA script URL at all — the known flow uses the fixed key at this
//!   point, which is why [`recaptcha_site_key`] falls back to it; and that `sid`/`code` may come
//!   back as either a JSON string or a number, which [`json_text`] tolerates.
//! - **Deliberate deviation**: a third, looser final-URL fallback,
//!   `(https?://[^/]+/download/[^<>"']+)`, is *not* implemented — it would happily match
//!   `https://rapidgator.net/download/captcha`, the very page being parsed, and hand back a
//!   silently wrong URL. A missing link is reported as `rapidgator.no_free_link` instead.

use serde::Deserialize;

mod parse;

use self::parse::{
    form_fields, form_with_id, js_number_var, js_string_var, number_after, open_tag, tag_attribute,
};
use plugin_common::html::{decode_entities, element_text, quoted_value};

/// The known fixed reCAPTCHA v2 site key of Rapidgator's free flow, used when the file page
/// carries no site key of its own. It is needed precisely because the key is only discoverable
/// on the *later* `/download/captcha` page, which cannot be fetched before the countdown has
/// run out.
pub(crate) const RECAPTCHA_SITE_KEY_FALLBACK: &str = "6LcSUAsUAAAAAKBeQQE893pf0Io66-mIeKWPl5yF";

/// The three JavaScript markers the file page carries for a free download.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimerMarkers {
    /// `var startTimerUrl = '...'` — absolute or page-relative.
    pub(crate) start_timer_url: String,
    /// `var fid = <digits>` — the numeric file id the countdown session is keyed on. Distinct
    /// from the link's own file id, which may be a 32-character hash.
    pub(crate) fid: u64,
    /// `var secs = <digits>` — the countdown the server enforces.
    pub(crate) wait_seconds: u64,
}

/// All three markers, or `None` when any of them is missing (then the page says why the
/// download is impossible, or the flow has changed).
#[must_use]
pub(crate) fn timer_markers(html: &str) -> Option<TimerMarkers> {
    Some(TimerMarkers {
        start_timer_url: js_string_var(html, "startTimerUrl")?,
        fid: js_number_var(html, "fid")?,
        wait_seconds: js_number_var(html, "secs")?,
    })
}

/// The reCAPTCHA v2 site key a page embeds: the widget's `data-sitekey` attribute first, then a
/// key carried in a `/recaptcha/` script or iframe URL. `None` leaves the caller to fall back to
/// [`RECAPTCHA_SITE_KEY_FALLBACK`].
pub(crate) use plugin_common::html::recaptcha_site_key;

/// Seconds this IP must wait before Rapidgator grants another free download, or `Some(0)` when
/// the page states a limit without naming a duration. `None` means no limit notice at all.
///
/// Covers the free-mode limit notices. The parallel-download and "already downloading" notices
/// could be read as the hoster being temporarily unavailable rather than an IP limit; both are
/// reported as an IP block here on purpose, so the
/// scheduler holds back every other free link of this hoster instead of spending a wait and a
/// paid captcha on each of them.
#[must_use]
pub(crate) fn ip_block_seconds(html: &str) -> Option<u64> {
    if let Some(minutes) = number_after(html, "Delay between downloads must be not less than") {
        return Some(minutes.saturating_mul(60));
    }
    // A 60-second wait, no reconnect needed — a duration is known, so it is reported.
    if html.contains("File is already downloading") {
        return Some(60);
    }
    let limited = [
        "You have reached your daily limit of downloads",
        "You have reached your daily downloads limit",
        "You have reached your hourly downloads limit",
        "You have reached your daily limit",
        // The known phrasing carries a backtick ("You can`t download more than 1 file at a time in
        // free mode."); the brief quotes a "not more than" variant. Both are matched by their
        // shared tail, so neither the backtick nor the drift matters.
        "download more than 1 file at a time",
        "download not more than 1 file at a time",
        "Wish to remove the restrictions?",
    ]
    .into_iter()
    .any(|marker| html.contains(marker));
    limited.then_some(0)
}

/// Whether the page rejected the captcha answer. "A captcha widget is still on the page" could
/// count as a rejection too; that case is covered here by the absence of a final link, which
/// reports `rapidgator.no_free_link`.
#[must_use]
pub(crate) fn is_wrong_captcha(html: &str) -> bool {
    html.contains("Please fix the following input errors")
        || html.contains("The verification code is incorrect")
}

/// A parsed `<form>`: where to post it and the fields it carries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Form {
    /// The form's `action` attribute, absent or empty meaning "post back to this page".
    pub(crate) action: Option<String>,
    pub(crate) fields: Vec<(String, String)>,
}

/// The `id="captchaform"` form on `/download/captcha`. `None` means no captcha is being asked
/// for, which means "no captcha needed" rather than an error.
#[must_use]
pub(crate) fn captcha_form(html: &str) -> Option<Form> {
    let form = form_with_id(html, "captchaform")?;
    Some(Form {
        action: tag_attribute(open_tag(form), "action").filter(|value| !value.is_empty()),
        fields: form_fields(form),
    })
}

/// Puts the solved captcha's token into both token fields, replacing any value the form
/// already carried for them.
#[must_use]
pub(crate) fn with_captcha_token(
    fields: &[(String, String)],
    token: &str,
) -> Vec<(String, String)> {
    const TOKEN_FIELDS: [&str; 2] = ["DownloadCaptchaForm[verifyCode]", "g-recaptcha-response"];
    let mut submitted: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| !TOKEN_FIELDS.contains(&name.as_str()))
        .cloned()
        .collect();
    for field in TOKEN_FIELDS {
        submitted.push((field.to_owned(), token.to_owned()));
    }
    submitted
}

/// The final download URL the captcha answer carries: the `?r=download/index&session_id=`
/// pattern first, then the `location.href = '...'` fallback. See the module doc for why a
/// third, much looser fallback is deliberately not implemented.
#[must_use]
pub(crate) fn final_download_link(html: &str) -> Option<String> {
    session_link(html).or_else(|| location_href(html))
}

fn session_link(html: &str) -> Option<String> {
    let at = html.find("r=download/index")?;
    let start = html[..at].rfind("http")?;
    let rest = &html[start..];
    let end = rest
        .find(|character: char| {
            matches!(character, '\'' | '"' | '<' | '>' | '\\') || character.is_whitespace()
        })
        .unwrap_or(rest.len());
    let link = decode_entities(&rest[..end]);
    link.contains("session_id=").then_some(link)
}

fn location_href(html: &str) -> Option<String> {
    const MARKER: &str = "location.href";
    let at = html.find(MARKER)?;
    let rest = html[at + MARKER.len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    quoted_value(rest)
        .map(|value| decode_entities(&value))
        .filter(|value| value.starts_with("http"))
}

/// `{"state":"started","sid":...}` / `{"state":"done","code":...}` — the two countdown answers.
/// `sid` and `code` are kept as raw JSON so a numeric value is tolerated (see the module doc).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TimerState {
    #[serde(default)]
    pub(crate) state: Option<String>,
    #[serde(default)]
    pub(crate) sid: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) code: Option<serde_json::Value>,
}

impl TimerState {
    /// Whether the answer reports `state` equal to `expected`, case-insensitively.
    #[must_use]
    pub(crate) fn is_state(&self, expected: &str) -> bool {
        self.state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case(expected))
    }

    /// The `state` value for a failure message, or a placeholder when the field is absent.
    #[must_use]
    pub(crate) fn state_text(&self) -> String {
        self.state
            .clone()
            .or_else(|| self.code.as_ref().and_then(json_text))
            .unwrap_or_else(|| "unknown".to_owned())
    }
}

/// Parses a countdown answer; `None` when the body is not the expected JSON object at all.
#[must_use]
pub(crate) fn timer_state(body: &[u8]) -> Option<TimerState> {
    serde_json::from_slice(body).ok()
}

/// A JSON scalar as text, so a `sid` that came back as a number is usable as one.
#[must_use]
pub(crate) fn json_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Whether `host` belongs to the hoster, so a parsed link is followed only when it does. The
/// final-URL patterns are host-agnostic (they accept any domain), and a link on a
/// foreign host must be refused rather than fetched.
#[must_use]
pub(crate) fn is_provider_host(host: &str) -> bool {
    const PRIMARY: &str = "rapidgator.net";
    host == PRIMARY || host.ends_with(&format!(".{PRIMARY}"))
}

/// The file name a Rapidgator link carries: `/file/<id>/<name>.html` -> `<name>`, without
/// percent-decoding — the decoded form is only ever a fallback
/// for the `Content-Disposition` name, so a percent-escaped segment is left as it stands rather
/// than pulling in a decoder for both targets.
#[must_use]
pub(crate) fn url_file_name(url: &url::Url) -> Option<String> {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())?;
    let name = last.strip_suffix(".html")?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Explains why a page carried none of the markers the flow needs, for the failure message.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    for marker in [
        "class=\"error\"",
        "class=\"err\"",
        "class=\"alert alert-danger\"",
    ] {
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
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(name, value)| (name, value)))
        .finish()
        .into_bytes()
}

#[cfg(test)]
#[path = "page/tests.rs"]
mod tests;
