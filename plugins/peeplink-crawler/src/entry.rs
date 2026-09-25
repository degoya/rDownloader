//! Reading one entry page.
//!
//! Two domains, two shapes, one rule. `peeplink.in` writes every hoster link as
//! `<a target="_blank" href="...">...</a>`; `alfalink.to` writes it as bare text between
//! `<br/>` tags inside `<article class="articless">`. Both were measured on 2026-09-21, and
//! the shape that covers both is to read the `<article>` and take every absolute address in
//! it, whether it stands in an attribute or in the text — on `peeplink.in` the attribute and
//! the link text are the same address, so one scan with de-duplication returns each link once.
//!
//! Nothing outside the `<article>` is read. That is the safety argument of this parser: the
//! surrounding page carries the login and register popups, the service's own navigation, a
//! jQuery from a CDN and an advertising loader, and none of that is a download.

use crate::target;

/// Most links taken from one entry page.
///
/// The widest measured page held 24. The cap is here so a page that is not an entry page at
/// all cannot turn one crawl into a thousand links; the host trims at 1.000 in any case.
const MAX_LINKS: usize = 1_000;

/// The marker `PrrpLinkIn.java` recognises a password-protected entry by, lowercase.
///
/// Matched on the whole page rather than on the `<article>`, because none of the seven
/// recorded pages carries it and where exactly the service puts the field is therefore not
/// measured. `name="pwd"` and `type="password"` are deliberately **not** used: every one of
/// the seven pages carries both, in the login and register popups.
const PASSWORD_MARKER: &str = "enter access password";

/// The body of the page's `<article>`, or `None` when it has none.
///
/// All seven recorded pages have exactly one, the `404` pages and the front page included —
/// which is why an `<article>` on its own says nothing about whether an entry exists.
#[must_use]
pub fn article(html: &str) -> Option<&str> {
    let lowered = html.to_ascii_lowercase();
    let open = lowered.find("<article")?;
    let body = open + lowered[open..].find('>')? + 1;
    let close = lowered[body..].find("</article>")? + body;
    Some(&html[body..close])
}

/// Whether the page is asking for the entry's access password.
#[must_use]
pub fn asks_for_password(html: &str) -> bool {
    html.to_ascii_lowercase().contains(PASSWORD_MARKER)
}

/// Every address the `<article>` names that leads away from the service, in page order.
///
/// De-duplicated, because `peeplink.in` writes each address twice — once in the `href` and
/// once as the link text — and a person who pasted one entry expects one link per file.
#[must_use]
pub fn links(article_body: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut cursor = 0;
    while cursor < article_body.len() {
        let Some(start) = next_scheme(&article_body[cursor..]) else {
            break;
        };
        let start = cursor + start;
        let end = article_body[start..]
            .find(is_terminator)
            .map_or(article_body.len(), |offset| start + offset);
        cursor = end.max(start + 1);
        let Some(url) = tidy(&article_body[start..end]) else {
            continue;
        };
        if target::is_service_url(&url) || found.contains(&url) {
            continue;
        }
        found.push(url);
        if found.len() >= MAX_LINKS {
            break;
        }
    }
    found
}

/// The offset of the next `http://` or `https://` in `text`, case-insensitively.
///
/// Compared as bytes rather than as string slices: a page comes from somebody else's server
/// and a slice taken at a fixed offset would panic on a multi-byte character right after an
/// `h`.
fn next_scheme(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(['h', 'H']) {
        let at = cursor + offset;
        let starts_with = |prefix: &[u8]| {
            bytes
                .get(at..at + prefix.len())
                .is_some_and(|window| window.eq_ignore_ascii_case(prefix))
        };
        if starts_with(b"http://") || starts_with(b"https://") {
            return Some(at);
        }
        cursor = at + 1;
    }
    None
}

/// Where an address stops: markup, quoting, whitespace, or a backslash.
fn is_terminator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '\\' | '`')
}

/// One candidate address, cleaned up, or `None` when it is not an address at all.
fn tidy(candidate: &str) -> Option<String> {
    // The only entity that occurs inside an address in a query string. Nothing else is
    // decoded: percent escapes belong to the address and must survive as they are.
    let url = candidate.replace("&amp;", "&");
    let url = url.trim_end_matches(['.', ',', ';', ':', '!']).to_owned();
    let (_, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    // A host with no dot is not a public address; the shortest real one still has two labels.
    if host.len() < 3 || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return None;
    }
    Some(url)
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
