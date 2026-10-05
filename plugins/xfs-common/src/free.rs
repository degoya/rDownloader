//! Page helpers for the XFS *free* (account-less) download flow, the counterpart to
//! [`crate::page`]'s premium `download2` helpers.
//!
//! Mirrors JDownloader's `XFileSharingProBasic.doFree`
//! (`svn_trunk/src/org/jdownloader/plugins/components/XFileSharingProBasic.java`): the file
//! page carries an `op=download1` form whose free-mode marker must be kept; posting it
//! yields a page with a countdown and usually a captcha; after solving the captcha and
//! waiting out the countdown, the `op=download2` form is posted and the answer is either the
//! file itself or a page carrying the direct link.
//!
//! Like the rest of this crate these are pure functions: each consuming plugin's native and
//! WebAssembly adapter drives the flow and performs the requests itself. The one exception is
//! [`FreeWords`]: the form post and the failures every XFS free flow reports, which the four
//! plugins carried byte for byte and which now take the plugin's codes instead (RD-1110-03,
//! audit R4).

use plugin_common::failure::{HttpError, diagnosed, free_limit};
use plugin_common::{
    CaptchaChallenge, Failure, FailureKind, HttpRequest, HttpResponse, PluginHost, WidgetChallenge,
};

/// Which captcha a page asks for, plus the site key needed to solve it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WidgetMarker {
    pub kind: WidgetKind,
    pub site_key: String,
}

/// The captcha widgets XFS installations embed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WidgetKind {
    RecaptchaV2,
    HCaptcha,
    Turnstile,
}

impl WidgetKind {
    /// The form field the widget's token is submitted in.
    #[must_use]
    pub const fn response_field(self) -> &'static str {
        match self {
            Self::RecaptchaV2 => "g-recaptcha-response",
            Self::HCaptcha => "h-captcha-response",
            Self::Turnstile => "cf-turnstile-response",
        }
    }

    /// The service's own name, for a message that has to tell a person what they are up against.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::RecaptchaV2 => "reCAPTCHA",
            Self::HCaptcha => "hCaptcha",
            Self::Turnstile => "Cloudflare Turnstile",
        }
    }
}

/// Turns the raw form fields into the free submission JDownloader sends: the premium marker
/// is dropped and `method_free` carries the button label the site expects.
///
/// The exact counterpart of [`crate::page::premium_form`], which drops `method_free` — that
/// asymmetry is why an account-less resolve could never work through the premium helper.
#[must_use]
pub fn free_form(fields: &[(String, String)], free_button: &str) -> Vec<(String, String)> {
    let mut free: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| name != "method_premium")
        .cloned()
        .collect();
    match free.iter_mut().find(|(name, _)| name == "method_free") {
        Some((_, value)) => *value = free_button.to_owned(),
        None => free.push(("method_free".to_owned(), free_button.to_owned())),
    }
    free
}

/// Adds a solved captcha's token to a form under the field its widget expects.
#[must_use]
pub fn with_captcha_token(
    fields: &[(String, String)],
    kind: WidgetKind,
    token: &str,
) -> Vec<(String, String)> {
    let field = kind.response_field();
    let mut submitted: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| name != field)
        .cloned()
        .collect();
    submitted.push((field.to_owned(), token.to_owned()));
    submitted
}

/// The captcha widget a page (or a single form's HTML) asks for, with its site key.
///
/// [`crate::page::has_captcha_challenge`] only answers *whether* a captcha is present, which
/// was all a plugin could act on while it had no way to solve one. Solving needs the widget
/// kind and the site key, so this reports both; the marker class is looked up first and the
/// `data-sitekey` attribute nearest to it wins.
#[must_use]
pub fn widget_marker(html: &str) -> Option<WidgetMarker> {
    let candidates = [
        ("g-recaptcha", WidgetKind::RecaptchaV2),
        ("h-captcha", WidgetKind::HCaptcha),
        ("cf-turnstile", WidgetKind::Turnstile),
    ];
    let (marker_at, kind) = candidates
        .into_iter()
        .filter_map(|(marker, kind)| html.find(marker).map(|at| (at, kind)))
        .min_by_key(|(at, _)| *at)?;
    let site_key = site_key_near(html, marker_at)?;
    Some(WidgetMarker { kind, site_key })
}

/// Finds the `data-sitekey` belonging to a marker: the next one after it, or — for templates
/// that put the attribute before the class — the closest one before it.
fn site_key_near(html: &str, marker_at: usize) -> Option<String> {
    const ATTRIBUTE: &str = "data-sitekey=";
    let after = html[marker_at..]
        .find(ATTRIBUTE)
        .and_then(|offset| attribute_value(&html[marker_at + offset + ATTRIBUTE.len()..]));
    if let Some(key) = after {
        return Some(key);
    }
    let before = html[..marker_at].rfind(ATTRIBUTE)?;
    attribute_value(&html[before + ATTRIBUTE.len()..])
}

fn attribute_value(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let end = value.find(quote)?;
    Some(value[..end].to_owned()).filter(|value| !value.is_empty())
}

/// Seconds a free download must wait, from the countdown markers the XFS base class parses
/// (`id="countdown_str"` with a nested `<span>`, or `class="seconds"`).
///
/// Sites whose countdown lives elsewhere (KatFile's `var estimated_time`, in tenths of a
/// second) keep their own parser and try it first.
#[must_use]
pub fn countdown_seconds(html: &str) -> Option<u64> {
    ["countdown_str", "class=\"seconds\"", "id=\"seconds\""]
        .into_iter()
        .filter_map(|marker| digits_after(html, marker))
        .find(|seconds| *seconds > 0)
}

/// The first run of digits appearing after `marker`, skipping markup in between.
fn digits_after(html: &str, marker: &str) -> Option<u64> {
    let rest = &html[html.find(marker)? + marker.len()..];
    // Stay inside the countdown element: a digit further down the page (a file size, a
    // year in the footer) must not be mistaken for the timer.
    let window = &rest[..rest.len().min(400)];
    let start = window.find(|c: char| c.is_ascii_digit())?;
    let digits: String = window[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// How long this IP must wait before the hoster grants another free download, in seconds.
///
/// Mirrors the phrasings JD's `checkErrors` parses ("You have to wait X minutes, Y seconds",
/// "You can download files up to ... only", the parallel-download notice). `None` means the
/// page carries no limit notice.
#[must_use]
pub fn ip_block_seconds(html: &str) -> Option<u64> {
    if let Some(seconds) = wait_phrase_seconds(html) {
        return Some(seconds);
    }
    // A limit without a stated duration still has to hold the hoster back; the scheduler
    // applies its own default when no duration is known.
    let limited = [
        "You have reached the download-limit",
        "You have reached the download limit",
        "you can download only one file at a time",
        "You can download only one file at a time",
        "Yo have reached the download limit",
    ]
    .into_iter()
    .any(|marker| html.contains(marker));
    limited.then_some(0)
}

/// Parses "You have to wait 2 hours, 30 minutes, 15 seconds" and its shorter forms.
fn wait_phrase_seconds(html: &str) -> Option<u64> {
    const MARKER: &str = "You have to wait";
    let start = html.find(MARKER)? + MARKER.len();
    let window = &html[start..html.len().min(start + 160)];
    unit_sum_seconds(window)
}

/// Sums a run of unit-tagged numbers — "2 hours, 30 minutes, 15 seconds" and its shorter forms —
/// from the front of `text`, stopping at the first number whose unit is not hours, minutes or
/// seconds. `None` when `text` carries no unit-tagged number at all.
///
/// Shared rather than private to [`wait_phrase_seconds`] because XFS installations state the same
/// duration behind differently worded sentences: the base class's "You have to wait ..." (which
/// [`ip_block_seconds`] anchors on) and, for example, FileJoker's "Please wait ... until the next
/// download" (`plugins/filejoker/src/page.rs`), which finds its own marker and then hands the
/// window after it to this function so both plugins agree on how the units add up.
#[must_use]
pub fn unit_sum_seconds(text: &str) -> Option<u64> {
    let mut total = 0_u64;
    let mut matched = false;
    let mut cursor = text;
    while let Some(digit_at) = cursor.find(|c: char| c.is_ascii_digit()) {
        let rest = &cursor[digit_at..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(value) = digits.parse::<u64>() else {
            break;
        };
        let after = rest[digits.len()..].trim_start().to_ascii_lowercase();
        let unit = if after.starts_with("hour") {
            3600
        } else if after.starts_with("minute") {
            60
        } else if after.starts_with("second") {
            1
        } else {
            0
        };
        if unit == 0 {
            break;
        }
        total += value.saturating_mul(unit);
        matched = true;
        cursor = &rest[digits.len()..];
    }
    matched.then_some(total)
}

/// Whether the page says the captcha answer was wrong, so a fresh challenge is worth one
/// more attempt (JD retries a rejected captcha rather than failing the link).
#[must_use]
pub fn is_wrong_captcha(html: &str) -> bool {
    [
        "Wrong captcha",
        "wrong captcha",
        "Skipped countdown",
        "Wrong Captcha",
        "verification code is incorrect",
    ]
    .into_iter()
    .any(|marker| html.contains(marker))
}

/// The codes and English texts a plugin reports its free flow's dead ends under; the failures
/// themselves are built here.
#[derive(Clone, Copy)]
pub struct FreeWords {
    /// The plugin's `http_error` code and text, for a status no page explains.
    pub http_error: HttpError,
    /// No free form on the page; the text takes the page's diagnosis.
    pub no_free_form: (&'static str, fn(&str) -> String),
    /// The last step yielded no direct link; the text takes the page's diagnosis.
    pub no_free_link: (&'static str, fn(&str) -> String),
    /// The IP may not start another free download yet; the text takes the stated wait.
    pub free_limit_reached: (&'static str, fn(Option<u64>) -> String),
}

impl FreeWords {
    /// Posts a free form back to the page it came from, as the page's own referer, asking for one
    /// byte so a file served in answer is recognised without downloading it.
    ///
    /// # Errors
    ///
    /// The host's failure, or the status classified under the plugin's `http_error`.
    pub async fn post_form<H: PluginHost>(
        &self,
        host: &H,
        url: &str,
        fields: &[(String, String)],
    ) -> Result<HttpResponse, Failure> {
        let response = host
            .http(
                HttpRequest::post(url.to_owned(), crate::page::encode_form(fields))
                    .with_header("Content-Type", "application/x-www-form-urlencoded")
                    .with_header("Referer", url.to_owned())
                    .with_header("Range", "bytes=0-0"),
            )
            .await?;
        crate::glue::ensure_http_status(&response, self.http_error.code, self.http_error.text)?;
        Ok(response)
    }

    /// Aborts a free flow when the page reports an IP limit ([`ip_block_seconds`]), because no
    /// amount of waiting or captcha solving helps until it expires.
    ///
    /// # Errors
    ///
    /// `IpBlocked` under the plugin's `free_limit_reached`, with `wait_seconds` when stated.
    pub fn free_page_failure(&self, html: &str) -> Result<(), Failure> {
        let (code, text) = self.free_limit_reached;
        free_limit(ip_block_seconds(html), code, text)
    }

    /// The page carries no free form this flow knows; `Permanent`, with the page's diagnosis.
    #[must_use]
    pub fn no_free_form(&self, html: &str) -> Failure {
        let (code, text) = self.no_free_form;
        diagnosed(
            FailureKind::Permanent,
            code,
            text,
            crate::page::diagnose(html),
        )
    }

    /// The flow's last page carries no direct link; `Permanent`, with the page's diagnosis.
    #[must_use]
    pub fn no_free_link(&self, html: &str) -> Failure {
        let (code, text) = self.no_free_link;
        diagnosed(
            FailureKind::Permanent,
            code,
            text,
            crate::page::diagnose(html),
        )
    }
}

/// Turns a page's captcha marker into the challenge the host solves.
#[must_use]
pub fn challenge_for(marker: &WidgetMarker, page_url: &str) -> CaptchaChallenge {
    let widget = WidgetChallenge {
        site_key: marker.site_key.clone(),
        page_url: page_url.to_owned(),
        invisible: false,
    };
    match marker.kind {
        WidgetKind::RecaptchaV2 => CaptchaChallenge::RecaptchaV2(widget),
        WidgetKind::HCaptcha => CaptchaChallenge::HCaptcha(widget),
        WidgetKind::Turnstile => CaptchaChallenge::Turnstile(widget),
    }
}

#[cfg(test)]
mod tests;
