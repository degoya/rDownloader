//! The page reader against the seven pages recorded on 2026-09-21.
//!
//! Every fixture here is a real answer of the live service; see
//! `tests/fixtures/README.md` for how each one was taken and what was removed. The counts
//! asserted below — 24, 1, 9 and 14 links — are the counts the measurement in
//! `docs/roadmap/jobs/110-17-die-uebrigen-protektoren.md` reports, so a change in this parser
//! that quietly loses a link fails here rather than in a package that is short two files.

use super::{article, asks_for_password, links};

const PEEPLINK_WIDE: &str = include_str!("../tests/fixtures/peeplink-0004ae96cef6.html");
const PEEPLINK_SINGLE: &str = include_str!("../tests/fixtures/peeplink-00013b965394.html");
const PEEPLINK_NOT_FOUND: &str = include_str!("../tests/fixtures/peeplink-unknown-404.html");
const PEEPLINK_DELETED: &str = include_str!("../tests/fixtures/peeplink-deleted-redirect.html");
const ALFALINK_NINE: &str = include_str!("../tests/fixtures/alfalink-02489255ba1048ae9d1328.html");
const ALFALINK_FOURTEEN: &str =
    include_str!("../tests/fixtures/alfalink-13e2cd9a35efcd6c6e4766.html");
const ALFALINK_NOT_FOUND: &str = include_str!("../tests/fixtures/alfalink-unknown-404.html");

/// Every link of the recorded page comes out, once each, and in page order.
#[test]
fn the_recorded_peeplink_entry_yields_all_twenty_four_links_once_each() {
    let body = article(PEEPLINK_WIDE).expect("an article");
    let found = links(body);
    assert_eq!(found.len(), 24, "{found:?}");
    assert_eq!(
        found[0],
        "https://rapidgator.net/file/9f3a418df007e8fb8ac2b8f3a62c46d8/227weji-thatt-macau.part1.rar.html"
    );
    assert_eq!(
        found[2],
        "http://uploaded.net/file/1fnqt1le/227weji-thatt-macau.part1.rar"
    );
    assert_eq!(
        found[23],
        "https://ddownload.com/y3nhpzppvyac/227weji-thatt-win86.part2.rar"
    );
    // The page writes each address twice, in the `href` and as the link text.
    let mut sorted = found.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), found.len(), "no address appears twice");
    // Three hosters and nothing else; no address of the service itself.
    assert!(
        found.iter().all(|url| {
            url.contains("rapidgator.net")
                || url.contains("uploaded.net")
                || url.contains("ddownload.com")
        }),
        "{found:?}"
    );
}

/// The narrow end of the same shape: one entry, one link.
#[test]
fn the_recorded_single_link_entry_yields_exactly_that_link() {
    let body = article(PEEPLINK_SINGLE).expect("an article");
    assert_eq!(links(body), vec!["http://uploaded.net/file/zr53zjoh"]);
}

/// The other HTML shape: `<article class="articless">` with the addresses as bare text.
#[test]
fn the_alfalink_shape_yields_its_nine_and_fourteen_links() {
    let nine = links(article(ALFALINK_NINE).expect("an article"));
    assert_eq!(nine.len(), 9, "{nine:?}");
    assert_eq!(nine[0], "https://streamtape.com/v/bGVqRGBmXKtPVoQ");
    assert_eq!(nine[1], "https://filemoon.to/d/vdpp24wl329j");
    assert_eq!(nine[8], "https://t.me/fpetopic");

    let fourteen = links(article(ALFALINK_FOURTEEN).expect("an article"));
    assert_eq!(fourteen.len(), 14, "{fourteen:?}");
    assert_eq!(fourteen[0], "https://streamtape.com/v/LawB0XP9WLURl83");
    assert!(
        fourteen
            .iter()
            .any(|url| url.starts_with("https://bt4gprx.com/magnet/")),
        "the torrent row is a link like any other: {fourteen:?}"
    );
}

/// A refusal page has an `<article>` too — so the element alone proves nothing.
#[test]
fn the_refusal_pages_carry_an_article_that_names_no_link() {
    for (name, page) in [
        ("peeplink 404", PEEPLINK_NOT_FOUND),
        ("alfalink 404", ALFALINK_NOT_FOUND),
        ("deleted entry, front page", PEEPLINK_DELETED),
    ] {
        let body = article(page).expect("an article");
        assert!(links(body).is_empty(), "{name}: {:?}", links(body));
    }
}

/// None of the seven recorded pages asks for an access password.
///
/// This is the finding the whole decision rests on: `name="pwd"` and `type="password"` are on
/// every one of them, in the login and register popups, and reading either as "protected"
/// would make every entry unreadable.
#[test]
fn no_recorded_page_asks_for_an_access_password() {
    for (name, page) in [
        ("wide entry", PEEPLINK_WIDE),
        ("single entry", PEEPLINK_SINGLE),
        ("peeplink 404", PEEPLINK_NOT_FOUND),
        ("deleted entry", PEEPLINK_DELETED),
        ("alfalink nine", ALFALINK_NINE),
        ("alfalink fourteen", ALFALINK_FOURTEEN),
        ("alfalink 404", ALFALINK_NOT_FOUND),
    ] {
        assert!(!asks_for_password(page), "{name}");
        assert!(page.contains("name=\"pwd\""), "{name} does carry the popup");
    }
}

/// The marker the password branch turns on is the one `PrrpLinkIn.java` uses, read case
/// insensitively.
///
/// This proves the recogniser and nothing more. **The password branch itself is untested**:
/// no protected entry was findable on either measuring day, so there is no page to record,
/// and inventing one would only show that the code agrees with itself (RD-110-17).
#[test]
fn the_password_marker_is_the_one_jdownloader_uses() {
    assert!(asks_for_password(
        "<input type=\"text\" value=\"Enter Access Password\" name=\"pwd\">"
    ));
    assert!(asks_for_password("VALUE=\"ENTER ACCESS PASSWORD\""));
    assert!(!asks_for_password("<input name=\"pwd\" type=\"password\">"));
}

/// Nothing outside the `<article>` is read, and that is what keeps the page's own furniture
/// — the CDN jQuery, the captcha widgets of the popups, the service's navigation — out.
#[test]
fn the_surrounding_page_is_never_read() {
    assert!(PEEPLINK_WIDE.contains("https://www.google.com/recaptcha/api.js"));
    assert!(PEEPLINK_WIDE.contains("https://hcaptcha.com/1/api.js"));
    let found = links(article(PEEPLINK_WIDE).expect("an article"));
    assert!(
        !found
            .iter()
            .any(|url| url.contains("google.com") || url.contains("hcaptcha.com")),
        "{found:?}"
    );
    // A page without an article at all is simply not an entry page.
    assert!(article("<html><body>nothing here</body></html>").is_none());
}

/// The scan reads text a server wrote, so the awkward shapes are decided rather than hoped.
#[test]
fn an_address_stops_where_markup_quoting_or_prose_does() {
    assert_eq!(
        links("see https://example.org/a.rar, and <a href='https://example.org/b.rar'>b</a>"),
        vec!["https://example.org/a.rar", "https://example.org/b.rar"]
    );
    // The one entity that occurs inside a query string.
    assert_eq!(
        links("<a href=\"https://example.org/g?a=1&amp;b=2\">x</a>"),
        vec!["https://example.org/g?a=1&b=2"]
    );
    // A host without a dot, and a bare scheme, are not addresses.
    assert!(links("http://localhost/x https:// http://./x").is_empty());
    // A multi-byte character right after an `h` must not end the scan.
    assert_eq!(
        links("h\u{e4}user https://example.org/x"),
        vec!["https://example.org/x"]
    );
    assert!(links("").is_empty());
}
