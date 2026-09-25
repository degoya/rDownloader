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
//! WebAssembly adapter drives the flow and performs the requests itself.

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

#[cfg(test)]
mod tests {
    use super::{
        WidgetKind, countdown_seconds, free_form, ip_block_seconds, is_wrong_captcha,
        unit_sum_seconds, widget_marker, with_captcha_token,
    };

    fn fields() -> Vec<(String, String)> {
        [
            ("op", "download1"),
            ("id", "abc123"),
            ("method_free", ""),
            ("method_premium", ""),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .to_vec()
    }

    /// The exact inverse of `page::premium_form`: keeping `method_free` is what makes an
    /// account-less download possible at all.
    #[test]
    fn free_form_keeps_the_free_marker_and_drops_the_premium_one() {
        let free = free_form(&fields(), "Free Download");
        assert!(free.iter().all(|(name, _)| name != "method_premium"));
        assert_eq!(
            free.iter().find(|(name, _)| name == "method_free"),
            Some(&("method_free".to_owned(), "Free Download".to_owned()))
        );

        let without = vec![("op".to_owned(), "download1".to_owned())];
        assert_eq!(free_form(&without, "Free Download").len(), 2);
    }

    #[test]
    fn a_captcha_token_replaces_any_existing_response_field() {
        let base = vec![("g-recaptcha-response".to_owned(), "stale".to_owned())];
        let submitted = with_captcha_token(&base, WidgetKind::RecaptchaV2, "fresh");
        assert_eq!(
            submitted,
            [("g-recaptcha-response".to_owned(), "fresh".to_owned())]
        );
        assert_eq!(
            with_captcha_token(&[], WidgetKind::Turnstile, "t")[0].0,
            "cf-turnstile-response"
        );
        assert_eq!(WidgetKind::HCaptcha.response_field(), "h-captcha-response");
    }

    #[test]
    fn each_widget_is_recognised_with_its_site_key() {
        let recaptcha = r#"<div class="g-recaptcha" data-sitekey="6Lc-abc"></div>"#;
        assert_eq!(
            widget_marker(recaptcha),
            Some(super::WidgetMarker {
                kind: WidgetKind::RecaptchaV2,
                site_key: "6Lc-abc".to_owned()
            })
        );

        let turnstile = r#"<div data-sitekey="0x4AAA" class="cf-turnstile"></div>"#;
        let marker = widget_marker(turnstile).expect("turnstile");
        assert_eq!(marker.kind, WidgetKind::Turnstile);
        assert_eq!(marker.site_key, "0x4AAA");

        let hcaptcha = r#"<div class="h-captcha" data-sitekey='hk-1'></div>"#;
        assert_eq!(widget_marker(hcaptcha).expect("hcaptcha").site_key, "hk-1");

        assert_eq!(widget_marker("<form></form>"), None);
        assert_eq!(
            widget_marker(r#"<div class="g-recaptcha"></div>"#),
            None,
            "a marker without a usable site key cannot be solved"
        );
    }

    #[test]
    fn the_countdown_is_read_from_the_usual_markers() {
        assert_eq!(
            countdown_seconds(r#"<span id="countdown_str">Wait <span id="xyz">45</span></span>"#),
            Some(45)
        );
        assert_eq!(
            countdown_seconds(r#"<span class="seconds">30</span>"#),
            Some(30)
        );
        assert_eq!(countdown_seconds("<p>no timer here</p>"), None);
        // A far-away digit must not be mistaken for the timer.
        let distant = format!(r#"<span class="seconds"></span>{}17"#, " ".repeat(500));
        assert_eq!(countdown_seconds(&distant), None);
    }

    #[test]
    fn a_stated_wait_is_summed_across_its_units() {
        assert_eq!(
            ip_block_seconds("<p>You have to wait 2 hours, 30 minutes, 15 seconds</p>"),
            Some(2 * 3600 + 30 * 60 + 15)
        );
        assert_eq!(
            ip_block_seconds("You have to wait 45 minutes till next download"),
            Some(45 * 60)
        );
    }

    /// A limit without a duration must still register, so the hoster is held back with the
    /// scheduler's own default instead of being retried at once.
    #[test]
    fn a_limit_without_a_duration_still_reports_a_block() {
        assert_eq!(
            ip_block_seconds("<p>You have reached the download-limit</p>"),
            Some(0)
        );
        assert_eq!(
            ip_block_seconds("<p>you can download only one file at a time</p>"),
            Some(0)
        );
        assert_eq!(ip_block_seconds("<p>Here is your file</p>"), None);
    }

    /// The unit summing [`ip_block_seconds`] uses, exposed for plugins whose site words the
    /// surrounding sentence differently (FileJoker's "Please wait ... until the next download").
    #[test]
    fn unit_sum_seconds_adds_hours_minutes_and_seconds_and_stops_at_an_unknown_unit() {
        assert_eq!(
            unit_sum_seconds(" 1 hour, 2 minutes, 3 seconds until the next download"),
            Some(3600 + 120 + 3)
        );
        assert_eq!(unit_sum_seconds(" 90 seconds"), Some(90));
        assert_eq!(unit_sum_seconds(" no numbers at all"), None);
        assert_eq!(
            unit_sum_seconds(" 5 minutes and 3 files"),
            Some(300),
            "a number whose unit is not a duration ends the run"
        );
    }

    #[test]
    fn a_rejected_captcha_is_recognised() {
        assert!(is_wrong_captcha("<div class=\"err\">Wrong captcha</div>"));
        assert!(is_wrong_captcha("the verification code is incorrect"));
        assert!(!is_wrong_captcha("<div>Download ready</div>"));
    }
}
