//! Target-independent HTML helpers for 1fichier's account-less ("free") website flow: the content
//! URL, the download form, the error markers the page can carry and the direct link it ends with.
//! Shared verbatim by `native/free.rs` and `guest/free.rs` so both adapters parse the same page the
//! same way and report byte-identical failures; like [`crate::api`] this module depends only on
//! `url`, which is available on every target.
//!
//! IMPL-VERIFY against JD's `svn_trunk/src/jd/plugins/hoster/OneFichierCom.java` (revision 53205,
//! the `$Revision$` tag on line 106) — `handleDownloadWebsite` lines 576-716,
//! `errorHandlingWebsite` lines 718-866, `isPasswordProtectedFileWebsite` lines 542-550,
//! `getContentURLWebsite` lines 286-293 and `isErrorNoFreeSlots` lines 868-874. Verified against
//! that source:
//!
//! - **The content URL keeps the uploader's domain.** `getContentURL` (lines 269-284) rebuilds
//!   `https://<the host the link came with>/?<fid>` and JD's own comment warns that normalizing to
//!   `1fichier.com` turns files restricted to `alterupload.com` (and the other aliases) offline.
//!   `getContentURLWebsite` then appends `&lg=en` to force English wording — which is why
//!   [`page_error`] only has to match JD's English markers. See [`content_url`].
//! - **The download form is the page's first form.** JD takes `br.getForm(0)` (line 628), removes
//!   the `save` field and puts `did=1` (lines 650-652); `dl_no_ssl=on` is only added in JD's
//!   opt-in FORCE_HTTP mode, which has no counterpart here. Every other hidden field the page
//!   carries (`adz` and friends) is submitted unchanged. See [`download_form`]/[`free_form`].
//! - **A password-protected file is detected by the form, not by prose.** JD's
//!   `isPasswordProtectedFileWebsite` looks for a form carrying a `pass` key (line 543). See
//!   [`is_password_protected`]; this plugin has no download password to send, so it reports the
//!   file rather than prompting the way JD does.
//! - **The direct link.** JD reads `<a href="...">Click here to download` and falls back to an
//!   absolute `*.1fichier.com`/`*.desfichiers.com` link (lines 696-699). See [`direct_link`].
//! - **The wait/limit markers.** `errorHandlingWebsite`'s IP-block block (lines 820-866) collects
//!   a stated `N minutes`, treats "IP Locked"/"Will be unlocked within 1h." as 60 minutes, and
//!   otherwise defaults to 5 minutes for any of a list of "you can only download one file at a
//!   time" phrasings. With `account == null` — this flow's only case — all of them end in
//!   `LinkStatus.ERROR_IP_BLOCKED`. See [`page_error`].
//!
//! Assumed, not verified against JD: the French markers of JD's list are not matched here (the
//! `&lg=en` above forces English wording, and JD's French phrasings carry accented characters that
//! Rust sources in this workspace avoid); a page that somehow arrives in French therefore falls
//! through to a `no_free_form`/`no_free_link` failure carrying the page's own wording as a
//! diagnosis, rather than being silently mis-read.

use url::Url;

/// Default cooldown for a limit the page states without a duration: JD's `defaultWaitMinutes = 5`
/// (line 828).
const DEFAULT_IP_BLOCK_SECONDS: u64 = 300;

/// "IP Locked" / "unusual usage" both carry JD's own one-hour cooldown (lines 826, 805).
const HOUR_SECONDS: u64 = 3600;

/// Phrasings that mean "this IP may not start another free download yet" without naming a
/// duration. Lowercased fragments of JD's `is_ip_blocked` list (lines 830-843).
const IP_BLOCK_MARKERS: &[&str] = &[
    "you must wait for another download",
    "you already downloading",
    "you can download only one file at a time",
    "please wait a few seconds before downloading new ones",
    "without premium status, you can download only one file at a time",
    "without premium, you can only download one file at a time",
    "without premium, you must wait between downloads",
    "without subscription, you can only download one file at",
];

/// Phrasings that mean "there is no free guest slot right now". JD's list carries the older
/// "Free download is temporarily limited due to high demand"; 1fichier has since moved to the
/// wording below, and while nothing matched, the flow walked past the notice into the
/// direct-link fallback (RD-109-36).
const NO_FREE_SLOTS_MARKERS: &[&str] = &[
    "all free guest slots are currently in use",
    "free slots are still available for registered users",
];

/// What a 1fichier page can say instead of handing over a download.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PageError {
    /// "File not found" — the file is gone.
    Offline,
    /// The file cannot be downloaded without an account at all.
    AccountRequired,
    /// "Free download is temporarily limited due to high demand" — the hoster ran out of free
    /// slots; nothing about this link is wrong.
    NoFreeSlots,
    /// This IP may not start another free download for the given number of seconds.
    IpBlocked(u64),
    /// A server-side problem worth retrying after the given number of seconds.
    ServerError(u64),
    /// "Your requests are too fast" — a short throttle (JD: 30 seconds, line 750).
    TooFast,
}

/// The URL the free flow fetches: the uploader's own domain, the file id, and `&lg=en` to force
/// English wording (JD's `getContentURLWebsite`).
pub(crate) fn content_url(url: &Url) -> Option<String> {
    Some(format!("{}&lg=en", crate::api::canonical_link(url)?))
}

/// A `<form>` on the page: where it posts to, and the fields it carries.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DownloadForm {
    /// The form's `action` attribute, empty when it posts back to the page itself.
    pub(crate) action: Option<String>,
    pub(crate) fields: Vec<(String, String)>,
}

/// The page's first form, the one JD submits (`br.getForm(0)`). `None` when the page carries no
/// form at all, which means the page is not a download page.
pub(crate) fn download_form(html: &str) -> Option<DownloadForm> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<form")?;
    let head_end = open + lower[open..].find('>')?;
    let close = head_end
        + lower[head_end..]
            .find("</form>")
            .unwrap_or(lower.len() - head_end);
    let action = attribute(&html[open..head_end], "action")
        .map(str::to_owned)
        .filter(|value| !value.is_empty());
    Some(DownloadForm {
        action,
        fields: inputs(&html[head_end..close]),
    })
}

/// Whether the form asks for a download password (JD: a form carrying a `pass` key).
pub(crate) fn is_password_protected(form: &DownloadForm) -> bool {
    form.fields
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("pass"))
}

/// The submission JD sends: every field the page carried, minus `save`, plus `did=1`.
pub(crate) fn free_form(form: &DownloadForm) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = form
        .fields
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("save"))
        .filter(|(name, _)| !name.eq_ignore_ascii_case("did"))
        .cloned()
        .collect();
    fields.push(("did".to_owned(), "1".to_owned()));
    fields
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(name, value)| (name, value)))
        .finish()
        .into_bytes()
}

/// What the page says instead of handing over a download, in JD's own precedence order.
pub(crate) fn page_error(html: &str) -> Option<PageError> {
    let lower = html.to_ascii_lowercase();
    if after_tag(&lower, "file not found") {
        Some(PageError::Offline)
    } else if after_tag(&lower, "software error") {
        Some(PageError::ServerError(600))
    } else if lower.contains("can't connect db") {
        Some(PageError::ServerError(300))
    } else if lower.contains("our services are in maintenance") {
        Some(PageError::ServerError(1200))
    } else if lower.contains("not possible to free unregistered users")
        || lower.contains("has reserved access to the subscribers")
    {
        Some(PageError::AccountRequired)
    } else if lower.contains("your requests are too fast") {
        Some(PageError::TooFast)
    } else if lower.contains("free download is temporarily limited due to high demand")
        || NO_FREE_SLOTS_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
    {
        Some(PageError::NoFreeSlots)
    } else {
        ip_block_seconds(&lower).map(PageError::IpBlocked)
    }
}

/// Seconds this IP must wait before another free download, or `None` when the page states no
/// limit at all.
fn ip_block_seconds(lower: &str) -> Option<u64> {
    if lower.contains("ip locked") || lower.contains("will be unlocked within 1h") {
        return Some(HOUR_SECONDS);
    }
    // JD's "Unusual usage detected" case (lines 803-816): a flat one-hour block.
    if lower.contains("the free offer is intended to") {
        return Some(HOUR_SECONDS);
    }
    if let Some(minutes) = minutes_after(lower, "you must wait") {
        return Some(minutes * 60);
    }
    IP_BLOCK_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
        .then_some(DEFAULT_IP_BLOCK_SECONDS)
}

/// The `N` of "you must wait [at least|up to] N minutes ...", tolerating JD's optional qualifier
/// wording without hard-coding it. Only a number actually followed by "minute" is accepted, so an
/// unrelated digit nearby cannot be mistaken for a duration.
fn minutes_after(lower: &str, marker: &str) -> Option<u64> {
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(marker) {
        let start = cursor + offset + marker.len();
        cursor = start;
        // Look only a little way past the marker: the qualifier is at most a couple of words.
        let head = &lower[start..lower.len().min(start + 48)];
        let Some(digits_at) = head.find(|character: char| character.is_ascii_digit()) else {
            continue;
        };
        let digits: String = head[digits_at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let tail = head[digits_at + digits.len()..].trim_start();
        if tail.starts_with("minute")
            && let Ok(minutes) = digits.parse::<u64>()
        {
            return Some(minutes);
        }
    }
    None
}

/// The download button's link on the page returned after posting the form. JD reads
/// `<a href="...">Click here to download` first and falls back to an absolute link on one of the
/// hoster's own content domains.
pub(crate) fn direct_link(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    // `to_ascii_lowercase` is byte-preserving, so offsets found in `lower` index `html` too.
    if let Some(marker) = lower.find("click here to download")
        && let Some(href) = href_before(html, marker)
    {
        return Some(href);
    }
    anchor_hrefs(html)
        .into_iter()
        .find(|link| is_content_host(link) && !link.contains("/register") && !link.ends_with('/'))
}

/// The last `href="..."` value that opens before `marker`, i.e. the anchor the marker sits in.
fn href_before(html: &str, marker: usize) -> Option<String> {
    let head = &html[..marker];
    let anchor = head.to_ascii_lowercase().rfind("href=")?;
    quoted_value(&html[anchor + "href=".len()..])
}

/// Every `href` carried by an anchor, in document order.
///
/// Deliberately not every `href` on the page: the head of a 1fichier page carries
/// `<link rel="icon" href="https://img.1fichier.com/favicon.ico">` and a stylesheet beside it.
/// `img.1fichier.com` passes [`is_content_host`], so scanning all hrefs made the favicon the
/// first match on any page that did not carry a download button -- 1150 bytes fetched, named
/// `download.bin` and booked as a finished 405 MB release (RD-109-36). JD reads the link out of
/// an `<a>`, and so does this.
fn anchor_hrefs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut links = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<a") {
        let open = cursor + offset;
        cursor = open + "<a".len();
        // `<a href=...`, never `<abbr`, `<area` or `<address`.
        if !lower[cursor..].starts_with(char::is_whitespace) {
            continue;
        }
        let Some(end) = lower[cursor..].find('>') else {
            break;
        };
        let tag_end = cursor + end;
        if let Some(value) = attribute(&html[open..tag_end], "href") {
            links.push(value.replace("&amp;", "&"));
        }
        cursor = tag_end;
    }
    links
}

fn quoted_value(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let end = value.find(quote)?;
    Some(value[..end].replace("&amp;", "&"))
}

/// Whether a link points at one of the hoster's own content domains, the fallback JD accepts.
fn is_content_host(link: &str) -> bool {
    let Ok(parsed) = Url::parse(link) else {
        return false;
    };
    parsed.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host.ends_with(".1fichier.com") || host.ends_with(".desfichiers.com")
    })
}

/// What the link page itself states about the file, before any download is attempted.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct StatedFile {
    pub(crate) name: String,
    pub(crate) size: u64,
}

/// The name and size 1fichier prints on the link page, in the `tier-name`/`tier-feat` pair of its
/// file card.
///
/// Read so the transfer has both before it starts: the name because the free flow's only other
/// source is the transfer's own `Content-Disposition`, which a page does not carry, and the size
/// because it is the one number that can contradict what the transfer actually receives
/// (RD-109-36).
pub(crate) fn stated_file(html: &str) -> Option<StatedFile> {
    let name = class_text(html, "tier-name")?;
    let size = class_text(html, "tier-feat").and_then(|text| parse_size(&text))?;
    Some(StatedFile { name, size })
}

/// The text of the first element carrying `class="<class>"`.
fn class_text(html: &str, class: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("\"{class}\"");
    let at = lower.find(&needle)?;
    let open = lower[..at].rfind('<')?;
    element_text(&html[open..], "<")
}

/// `"405.44 MB"` as bytes. 1fichier prints binary units under the decimal names, so `MB` is
/// 1024 * 1024 -- the same reading JD uses. Parsed through an integer scale rather than a float
/// so the result is exactly reproducible.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = text.split_at(split);
    let factor: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "B" => 1,
        "KB" | "KIB" => 1024,
        "MB" | "MIB" => 1024 * 1024,
        "GB" | "GIB" => 1024 * 1024 * 1024,
        "TB" | "TIB" => 1024_u64.pow(4),
        _ => return None,
    };
    let (whole, fraction) = match number.trim().split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (number.trim(), ""),
    };
    // Two decimals is what the site prints; more would only add noise below the unit.
    let fraction: String = fraction.chars().take(2).collect();
    let scale = 10_u64.pow(fraction.len() as u32);
    let whole: u64 = whole.trim().parse().ok()?;
    let fraction: u64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let scaled = whole.checked_mul(scale)?.checked_add(fraction)?;
    scaled.checked_mul(factor).map(|bytes| bytes / scale)
}

/// Explains why a page came back instead of a file, so an unmapped page still reaches the user in
/// its own words instead of as a bare "something went wrong".
pub(crate) fn diagnose(html: &str) -> String {
    element_text(html, "<title")
        .or_else(|| element_text(html, "<h1"))
        .unwrap_or_else(|| "unexpected 1fichier page".to_owned())
}

/// The text of the first element opened by `marker`, whitespace-collapsed and length-capped.
fn element_text(html: &str, marker: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find(marker)?;
    let text_at = open + lower[open..].find('>')? + 1;
    let end = text_at + lower[text_at..].find('<')?;
    let text: String = html[text_at..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(160) {
        Some((cut, _)) => text[..cut].to_owned(),
        None => text,
    })
}

/// Every `<input name=...>` of a form, with its value (empty when it carries none).
fn inputs(form: &str) -> Vec<(String, String)> {
    let lower = form.to_ascii_lowercase();
    let mut fields = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<input") {
        let tag_start = cursor + offset;
        let Some(length) = lower[tag_start..].find('>') else {
            break;
        };
        let tag = &form[tag_start..tag_start + length];
        cursor = tag_start + length;
        if let Some(name) = attribute(tag, "name") {
            fields.push((
                name.to_owned(),
                attribute(tag, "value").unwrap_or_default().to_owned(),
            ));
        }
    }
    fields
}

/// One attribute of an HTML tag, quoted or bare.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(name) {
        let start = cursor + offset;
        cursor = start + name.len();
        let preceded_by_space = start == 0
            || lower[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = tag[cursor..].trim_start();
        if !preceded_by_space || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return rest.split(|c: char| c.is_whitespace() || c == '>').next();
        }
        let value = &rest[1..];
        return value.find(quote).map(|end| &value[..end]);
    }
    None
}

/// Whether `needle` appears as element text rather than anywhere at all — JD writes these markers
/// as `>\s*<phrase>` so a stray mention in a script or a comment cannot trip them.
fn after_tag(lower: &str, needle: &str) -> bool {
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(needle) {
        let start = cursor + offset;
        cursor = start + needle.len();
        if lower[..start].trim_end().ends_with('>') {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "page/tests.rs"]
mod tests;
