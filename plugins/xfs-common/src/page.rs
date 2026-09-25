//! HTML helpers for the XFS `download2` premium form flow, shared by every consuming plugin's
//! native and WebAssembly adapters. Generalized verbatim from `plugins/ddownload/src/page.rs`
//! (pre-Task-11): the two functions whose text is XFS-site-specific (the download-form's `op`
//! marker and the premium submit button's label, plus the direct-link domain filter) now take
//! that text as a parameter instead of hardcoding ddownload's own value; every consuming plugin's
//! thin `page.rs` wrapper supplies its own site's parameterization (ddownload's unchanged;
//! KatFile's is the same defaults — see `plugins/katfile/src/page.rs`, no site-specific deviation
//! was found for these markers).

/// The raw `<form>...</form>` substring carrying `op=<op_value>` (the same boundary
/// [`download_form`] parses fields from), for callers that need to scope a plain-text scan (e.g.
/// a captcha-marker check) to the form itself rather than the whole page — a widget elsewhere on
/// the page (a login modal, a site-wide banner) must not be mistaken for one blocking this form.
#[must_use]
pub fn form_html<'a>(html: &'a str, op_value: &str) -> Option<&'a str> {
    let (start, end) = form_bounds(html, op_value)?;
    Some(&html[start..end])
}

fn form_bounds(html: &str, op_value: &str) -> Option<(usize, usize)> {
    let marker = format!("value=\"{op_value}\"");
    let marker_at = html.find(&marker)?;
    let start = html[..marker_at].rfind("<form")?;
    let end = marker_at + html[marker_at..].find("</form>")?;
    Some((start, end))
}

/// The `action` attribute of the `op=<op_value>` form, if it declares one.
///
/// XFS writes an absolute URL here for the login form and omits it entirely on some file
/// pages, where the form posts back to the page it came from; the caller decides which
/// fallback applies.
#[must_use]
pub fn form_action<'a>(html: &'a str, op_value: &str) -> Option<&'a str> {
    let (start, end) = form_bounds(html, op_value)?;
    let tag_end = start + html[start..end].find('>')?;
    attribute(&html[start..tag_end], "action")
}

/// Hidden form fields of the `op=<op_value>` form on a file page (e.g. `op=download2`).
/// `op_value` is compared as it appears inside a `value="..."` HTML attribute, e.g.
/// `"download2"` for the marker `value="download2"`.
#[must_use]
pub fn download_form(html: &str, op_value: &str) -> Option<Vec<(String, String)>> {
    let (start, end) = form_bounds(html, op_value)?;
    let form = &html[start..end];
    let mut fields = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = form[cursor..].find("<input") {
        let tag_start = cursor + offset;
        let tag_end = tag_start + form[tag_start..].find('>')?;
        let tag = &form[tag_start..tag_end];
        cursor = tag_end;
        if attribute(tag, "type").is_some_and(|kind| !kind.eq_ignore_ascii_case("hidden")) {
            continue;
        }
        if let Some(name) = attribute(tag, "name") {
            fields.push((
                name.to_owned(),
                attribute(tag, "value").unwrap_or_default().to_owned(),
            ));
        }
    }
    (!fields.is_empty()).then_some(fields)
}

/// Turns the raw `download2` fields into the premium submission JDownloader sends: the free-mode
/// marker is dropped and `method_premium` carries `premium_button`, the button label the site's
/// XFS installation expects for a premium session (ddownload's default: `"Premium Download"`).
#[must_use]
pub fn premium_form(fields: &[(String, String)], premium_button: &str) -> Vec<(String, String)> {
    let mut premium: Vec<(String, String)> = fields
        .iter()
        .filter(|(name, _)| name != "method_free")
        .cloned()
        .collect();
    match premium
        .iter_mut()
        .find(|(name, _)| name == "method_premium")
    {
        Some((_, value)) => *value = premium_button.to_owned(),
        None => premium.push(("method_premium".to_owned(), premium_button.to_owned())),
    }
    premium
}

/// Whether `html` is a login page: it carries the XFS sign-in form, the one whose hidden `op`
/// field says `login`.
///
/// Until RD-108-28 the navigation's `href="/login"` link counted too, and that link sits in the
/// header of every page a guest is shown — DDownload's file page, measured on 2026-09-17,
/// carries it four times: twice in the header, twice in the register and password-reset modals
/// that ship on every page. So every page that merely lacked the form we were looking for was
/// reported as "the page requires a login", to free and premium users alike, and both went
/// looking for a cookie problem they did not have. The `LoginModal` script ships on every page
/// as well. Only the form itself says that the site is asking for credentials *here*.
///
/// Exposed separately from [`diagnose`] (which checks this after the page's own message) so a
/// caller can distinguish "this page specifically is a login wall" from `diagnose`'s other, more
/// generic fallback text — e.g. to report a distinct failure code for an expired cookie session.
#[must_use]
pub fn is_login_wall(html: &str) -> bool {
    download_form(html, "login").is_some_and(|fields| {
        fields
            .iter()
            .any(|(name, value)| name == "op" && value == "login")
    })
}

/// Whether `html` was rendered for a visitor who is not signed in, as far as the header can
/// tell: it offers a `href="/login"` link. The exact marker [`is_login_wall`] used to rest on,
/// kept apart from it on purpose. As a *positive* signal it is worthless — every guest page
/// carries it, so it cannot say "this page asks for a login" — and it may sit in a modal that
/// ships on signed-in pages too, so its presence does not disprove a session either. What it
/// is good for is the explanation once [`shows_signed_in`] has said no: a page with the guest
/// header is the homepage a refused sign-in redirects to, not a maintenance page or an
/// interstitial (RD-108-28 review).
#[must_use]
pub fn shows_guest_header(html: &str) -> bool {
    html.contains("href=\"/login\"")
}

/// Whether `html` was rendered for a signed-in visitor: it offers a sign-out link or label.
///
/// The positive marker, and the measured one. DDownload's guest file page of 2026-09-17
/// contains `href="/login"` four times — twice in the header's "not logged in" branch, twice in
/// the register and password-reset modals, whose presence on a signed-in page nobody has ruled
/// out — so a session check keyed on the *absence* of the login link could report a successful
/// sign-in as failed. It contains `logout`, in any case, exactly once: the stylesheet comment
/// `/* Narrow dropdown for Dashboard/Logout */` at line 390, and zero times once the decoration
/// is stripped (`log out`, `log-out`, `log_out`: zero before and after). So the rule is one
/// rule: on the page stripped by [`without_style_script_and_comments`], case-insensitively,
/// `logout` with or without a separator — which covers `/?op=logout`, `/logout`, `?op=LOGOUT`
/// and a plain "Log out" label alike, whichever spelling the signed-in page turns out to use.
///
/// Two things make that rule safe rather than merely broad, and both were measured, not
/// assumed. It reads the marker as a word ([`contains_word`]), because the same page carries
/// `dialog` sixteen times and a substring search would have called `dialog outside` a session.
/// And it reads it on the stripped page, because a stylesheet and a comment are where the word
/// appears without the thing it marks. Across all three captures of 2026-09-17 — the file page,
/// the error page and the 209 KB full capture — the count of word-boundary hits of any spelling
/// on a guest page is zero.
///
/// It is the marker [`crate::login::login_outcome`], FileJoker's and DDownload's cookie session
/// checks believe; a page without it is not a session, whatever else it shows. The direction
/// matters: a false negative reports a working account as invalid, which is annoying; a false
/// positive reports a lapsed session as healthy and spends a captcha on it.
#[must_use]
pub fn shows_signed_in(html: &str) -> bool {
    let text = without_style_script_and_comments(html).to_ascii_lowercase();
    ["logout", "log out", "log-out", "log_out"]
        .into_iter()
        .any(|marker| contains_word(&text, marker))
}

/// What a page says about the visitor's session — in the three states it can really tell apart.
///
/// [`shows_signed_in`] is deliberately a single positive marker, and that makes it a good
/// predicate and a bad verdict: everything that is not the sign-out link reads as "not a
/// session", including pages that say nothing about the matter at all. A caller that turns
/// that straight into `AccountInvalid` tells the user their working premium account is
/// broken as soon as the site answers with a Cloudflare interstitial, a maintenance notice,
/// a layout whose sign-out label is localized past the English marker, or a header a script
/// draws after [`without_style_script_and_comments`] has removed it.
///
/// So the two negative answers are kept apart. [`SessionVerdict::Guest`] is the one the
/// site's own markup supports: the login form, or the guest header a lapsed session is
/// redirected to. [`SessionVerdict::Unknown`] is every other page, and it is not an account
/// problem — it is a page this code does not recognize, which is a reason to retry, not a
/// reason to send the user after their cookies (RD-108-28 review).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionVerdict {
    /// The page offers the sign-out link: a session, positively.
    SignedIn,
    /// The page is one the site shows a visitor who is not signed in - the login form itself,
    /// or the guest header. The session is genuinely not there.
    Guest,
    /// Neither marker is present. The page arrived and settles nothing.
    Unknown,
}

/// Classifies `html` by what it shows about the visitor's session. See [`SessionVerdict`] for
/// why the two negative answers must not be collapsed into one.
#[must_use]
pub fn session_verdict(html: &str) -> SessionVerdict {
    if shows_signed_in(html) {
        return SessionVerdict::SignedIn;
    }
    if is_login_wall(html) || shows_guest_header(html) {
        return SessionVerdict::Guest;
    }
    SessionVerdict::Unknown
}

/// What a session probe found, carrying the diagnosis the failure message needs.
///
/// [`session_verdict`] answers the question; this answers it in the shape the call site has,
/// so none of the three XFS plugins writes that `match` for itself and none of them can read
/// the diagnosis off a different page than the one it judged. The distinction the name makes
/// is the one RD-120-13 was reported for: an account check that merely counted the cookies in
/// the jar reported "8 cookie(s) loaded" for a session the site had already forgotten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionState {
    /// The page offered the sign-out link: the session is live.
    Active,
    /// The site answered with one of its own guest pages. The session is genuinely gone -
    /// which, for an imported cookie jar, is what an expired session looks like.
    Expired(String),
    /// The page settles nothing, so nothing is decided against the account.
    Unconfirmed(String),
}

/// Classifies the body of a session probe. See [`SessionVerdict`] for why the two negative
/// answers must not be collapsed into one.
#[must_use]
pub fn classify_session(html: &str) -> SessionState {
    match session_verdict(html) {
        SessionVerdict::SignedIn => SessionState::Active,
        SessionVerdict::Guest => SessionState::Expired(diagnose(html)),
        SessionVerdict::Unknown => SessionState::Unconfirmed(diagnose(html)),
    }
}

/// Whether `text` contains `word` as a word rather than inside a longer one.
///
/// The marker is short and English, so a plain substring search is a trap a hoster page walks
/// into on its own: this page carries `dialog` sixteen times, and `dialog outside` would have
/// read as a session. A letter or digit on either side disqualifies a hit; punctuation, a tag
/// bracket or whitespace does not. That deliberately lets identifier separators through, so
/// `op=logout`, `logOut()` and `class="dialog-logout"` all count — the first two are how a
/// signed-in page really spells it, and the third is a class a signed-in page is the one to
/// carry. The measured guest pages contain none of the three.
fn contains_word(text: &str, word: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|char| !char.is_alphanumeric());
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|char| !char.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        // `end`, not `start + 1`: none of the markers overlaps itself, so nothing is skipped,
        // and `end` is a char boundary for a marker that is one day not pure ASCII.
        from = end;
    }
    false
}

/// `html` with every `<style>`, `<script>` and `<!-- -->` block removed, tags matched in any
/// case — the three places a marker's text appears without the thing it marks: a stylesheet
/// rule naming a widget's class, a loader script, and a comment that narrates what the markup
/// beside it would mean. The comments are not hypothetical: the capture of 2026-09-17 keeps 55
/// of them after the tags are stripped, and one reads
/// `<!-- Language selector (logged out: after Konto erstellen) -->` — a site that writes about
/// its own auth state in comments is one `<!-- Logout dropdown -->` away from a guest page that
/// reads as a session. A block that never closes is dropped to the end of the document. What
/// remains is what [`shows_signed_in`] and DDownload's widget scan look at.
#[must_use]
pub fn without_style_script_and_comments(html: &str) -> String {
    // ASCII lower-casing keeps every byte offset, so positions found in `lower` index `html`.
    let lower = html.to_ascii_lowercase();
    let mut kept = String::with_capacity(html.len());
    let mut cursor = 0;
    loop {
        let next = [
            ("<style", "</style>"),
            ("<script", "</script>"),
            ("<!--", "-->"),
        ]
        .into_iter()
        .filter_map(|(open, close)| lower[cursor..].find(open).map(|at| (cursor + at, close)))
        .min_by_key(|(at, _)| *at);
        let Some((at, close)) = next else {
            kept.push_str(&html[cursor..]);
            return kept;
        };
        kept.push_str(&html[cursor..at]);
        cursor = match lower[at..].find(close) {
            Some(end) => at + end + close.len(),
            None => html.len(),
        };
    }
}

/// The page's own message, when it carries one in the XFS engine's `err` or
/// `alert alert-danger` box. What [`diagnose`] reports first; exposed for callers that need
/// to know whether the page said anything at all before choosing their own wording.
#[must_use]
pub fn page_message(html: &str) -> Option<String> {
    element_text(html, "class=\"err\"")
        .or_else(|| element_text(html, "class=\"alert alert-danger\""))
        .filter(|text| !text.is_empty())
}

/// Explains why an HTML page came back instead of a file, for error messages. Fully shared: this
/// HTML shape (the `LoginModal` script, `class="err"`/`class="alert alert-danger"` message boxes,
/// the page `<title>`) comes from the XFS engine template itself, not from any one site's theme.
#[must_use]
pub fn diagnose(html: &str) -> String {
    // The page's own words come first. This used to be the other way round, and it cost a
    // release: DDownload put a Turnstile widget on its login form, answered every sign-in with
    // a page saying "Wrong captcha", and because that page is also a login wall the message
    // that reached the user talked about cookies instead — sending them after a problem they
    // did not have while the real explanation sat unread in the same document.
    if let Some(message) = page_message(html) {
        return format!("page message: {message}");
    }
    if is_login_wall(html) {
        return "the page requires a login - the cookies were not sent or do not belong to a logged-in session".to_owned();
    }
    match element_text(html, "<title>").filter(|text| !text.is_empty()) {
        Some(title) => format!("page \"{title}\" contains no premium link"),
        None => "the response page contains no premium link".to_owned(),
    }
}

/// Text content following `marker` up to the next closing tag, tags stripped,
/// whitespace collapsed and capped at 160 characters.
fn element_text(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let rest = match rest.find('>') {
        Some(end) if marker.starts_with("class=") => &rest[end + 1..],
        _ => rest,
    };
    // Stops at the first closing tag that is not nested inline markup (<b>, <i>, …).
    let mut text = String::new();
    let mut depth = 0_usize;
    let mut cursor = rest;
    while let Some(open) = cursor.find('<') {
        text.push_str(&cursor[..open]);
        let after = &cursor[open + 1..];
        let Some(close) = after.find('>') else { break };
        let tag = &after[..close];
        if tag.starts_with('/') {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if !tag.ends_with('/') {
            depth += 1;
        }
        cursor = &after[close + 1..];
    }
    if !cursor.contains('<') {
        text.push_str(cursor);
    }
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(collapsed.chars().take(160).collect())
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(name, value)| (name, value)))
        .finish()
        .into_bytes()
}

/// Finds the premium direct link on the page returned after submitting the form.
///
/// Prefers links whose path ends with one of `hints` (file name or code) and falls back to any
/// absolute link on `domain` that is not a navigation target. `domain` is the bare hostname to
/// require in the link (e.g. `"ddownload.com"`, matching a hint like `fs7.ddownload.com`).
#[must_use]
pub fn direct_link(html: &str, hints: &[&str], domain: &str) -> Option<String> {
    direct_link_any(html, hints, &[domain])
}

/// Finds the direct link when a site delivers from more than one domain.
///
/// XFS installations often hand the file to a separate CDN (DDownload's `zeuscdn.org`), so
/// the answer page's link is not on the site's own domain at all. Hosts are compared after
/// parsing the URL rather than by substring, because a delivery host may carry a port
/// (`https://eu-hydra5.zeuscdn.org:183/d/...`), which a `"<domain>/"` match would miss.
#[must_use]
pub fn direct_link_any(html: &str, hints: &[&str], domains: &[&str]) -> Option<String> {
    let links = hrefs(html, domains);
    links
        .iter()
        .find(|link| {
            let path = link.split('?').next().unwrap_or(link);
            hints
                .iter()
                .any(|hint| !hint.is_empty() && path.ends_with(hint))
        })
        .or_else(|| links.iter().find(|link| link.contains("/d/")))
        .cloned()
}

/// Whether `host` is one of `domains` or a subdomain of one.
fn host_matches(host: &str, domains: &[&str]) -> bool {
    domains.iter().any(|domain| {
        let domain = domain.trim_start_matches("*.");
        host.eq_ignore_ascii_case(domain)
            || host
                .len()
                .checked_sub(domain.len() + 1)
                .is_some_and(|start| {
                    host.as_bytes()[start] == b'.' && host[start + 1..].eq_ignore_ascii_case(domain)
                })
    })
}

fn hrefs(html: &str, domains: &[&str]) -> Vec<String> {
    let mut links = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("href=") {
        let start = cursor + offset + "href=".len();
        cursor = start;
        let Some(quote) = html[start..].chars().next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            continue;
        }
        let value_start = start + 1;
        let Some(length) = html[value_start..].find(quote) else {
            break;
        };
        let value = html[value_start..value_start + length].replace("&amp;", "&");
        let on_domain = url::Url::parse(&value).is_ok_and(|url| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| host_matches(host, domains))
        });
        if on_domain {
            links.push(value);
        }
    }
    links
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut cursor = 0;
    while let Some(offset) = tag[cursor..].find(name) {
        let start = cursor + offset;
        cursor = start + name.len();
        let preceded_by_space = start == 0
            || tag[..start]
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

/// XFS captcha markers found on a `download2` premium form or its containing page: KatFile's JD
/// plugin (`KatfileCom.findFormDownload2Premium` -> `XFileSharingProBasic.handleCaptcha`,
/// `svn_trunk/src/org/jdownloader/plugins/components/XFileSharingProBasic.java:2990-3119`)
/// conditionally requires solving a reCaptchaV2/hCaptcha/Cloudflare-Turnstile challenge before the
/// premium form is accepted; this crate has no interactive captcha-solving capability, so a
/// consuming plugin that can hit this flow (KatFile; ddownload's own JD plugin has no such
/// override and never calls this) checks for these markers and reports `NeedsCaptcha` instead of
/// posting a form the server will reject anyway. Not used by ddownload (see
/// `plugins/ddownload/src/native.rs`): ddownload's premium flow never reaches a captcha
/// challenge, so leaving this function uncalled on that path changes nothing about its behavior.
/// As of the domain-mismatch fix, KatFile scopes this scan to [`form_html`]'s substring rather
/// than the whole page (see `plugins/katfile/src/native/api.rs`'s module doc) — a page-wide scan
/// risked aborting a resolve over an unrelated widget (a login modal, a site-wide banner).
#[must_use]
pub fn has_captcha_challenge(html: &str) -> bool {
    html.contains("data-sitekey=\"")
        || html.contains("g-recaptcha")
        || html.contains("h-captcha")
        || html.contains("cf-turnstile")
}

#[cfg(test)]
mod tests;
