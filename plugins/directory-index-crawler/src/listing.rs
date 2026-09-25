//! Reading one directory listing page.
//!
//! Four servers, one shape. What Apache's `mod_autoindex`, nginx's `autoindex`, lighttpd's
//! `mod_dirlisting` and Caddy's `file_server browse` have in common is not their markup — it
//! is that every entry is an `<a href>` naming something directly under the current path, and
//! that the way back up is marked: `href="../"` for three of them, the words "Parent
//! Directory" for Apache. That is the whole recogniser, and it is why this is one plugin
//! rather than four.
//!
//! What is *not* trusted is the href. Only links that resolve strictly one level below the
//! page's own path are kept, which drops the parent link, the sort links, the links to
//! another host and anything a page merely happens to contain.

use crate::target::{self, Address};

/// One entry a listing named.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Directory {
        url: String,
        name: String,
    },
    File {
        url: String,
        name: String,
        size: Option<u64>,
    },
}

/// Whether this page is a directory listing at all.
///
/// A crawler that claims by shape has to be able to say "I was wrong" (RD-107-05): anything
/// answering here with `false` becomes an `unsupported` refusal, after which the selection
/// carries the address on to the next crawler instead of ending it.
#[must_use]
pub fn is_index(html: &str) -> bool {
    let lowered = html.to_ascii_lowercase();
    // nginx, lighttpd and Caddy all write the way back up as a bare `../`.
    lowered.contains("href=\"../\"")
        || lowered.contains("href='../'")
        // Apache, and every listing whose title still says what it is.
        || lowered.contains("index of")
        || lowered.contains("parent directory")
        // Python's `http.server`, which is what a great many one-off shares are.
        || lowered.contains("directory listing for")
}

/// Every entry this page names directly below `page`.
///
/// Duplicates are kept: a listing that names the same file twice is the server's business,
/// and the walk de-duplicates by address anyway.
#[must_use]
pub fn entries(page: &Address, html: &str) -> Vec<Entry> {
    let mut found = Vec::new();
    for (href, tail) in hrefs(html) {
        let Some(child) = child_path(page, &href) else {
            continue;
        };
        let name = target::decode(child.trim_end_matches('/'));
        if name.is_empty() {
            continue;
        }
        let url = format!("{}{}{}", page.origin(), page.path, child);
        if child.ends_with('/') {
            found.push(Entry::Directory { url, name });
        } else {
            found.push(Entry::File {
                url,
                name,
                size: trailing_size(&tail),
            });
        }
    }
    found
}

/// Every `href="..."` in the page, with the text that follows the link's row.
///
/// A hand-written scan rather than an HTML parser: the component links nothing outside
/// `rdownloader:plugin`, and what is needed here is one attribute and the bytes after it.
/// The page is lowered once and scanned by byte index — `to_ascii_lowercase` changes no
/// byte's length, so the two strings stay aligned — because lowering the remainder on every
/// link turns a large listing into a quadratic walk, and a crawl has a fuel budget.
fn hrefs(html: &str) -> Vec<(String, String)> {
    let lowered = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lowered[cursor..].find("href") {
        let mut index = cursor + offset + "href".len();
        cursor = index;
        if out.len() >= MAX_LINKS_PER_PAGE {
            break;
        }
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            continue;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let quote = match bytes.get(index) {
            Some(&b'"') => '"',
            Some(&b'\'') => '\'',
            _ => continue,
        };
        let start = index + 1;
        let Some(length) = html[start..].find(quote) else {
            continue;
        };
        let href = html[start..start + length].to_owned();
        let tail = &html[start + length + 1..];
        // The rest of this listing row: enough to find a byte count, never more.
        let row = tail.split('\n').next().unwrap_or("");
        let row: String = row.chars().take(160).collect();
        out.push((href, row));
        cursor = start + length + 1;
    }
    out
}

/// Most links read from one page. A listing larger than this is past the walk's file limit
/// anyway; the cap is here so a page that is not a listing at all cannot cost a whole crawl.
const MAX_LINKS_PER_PAGE: usize = 5_000;

/// The one path segment `href` names directly below `page`, or `None`.
///
/// This is the whole safety argument of the parser. A link is kept only when it resolves to
/// exactly one level under the page's own path on the page's own origin — so the parent
/// link, the sort links, an absolute link to another host and a link three levels down all
/// drop out, and the walk's depth limit is the only way further in.
fn child_path(page: &Address, href: &str) -> Option<String> {
    let href = href.trim();
    let href = href.split('#').next().unwrap_or("");
    if href.is_empty() || href.contains('?') || href.starts_with("//") {
        return None;
    }
    let path = if href.contains("://") {
        let target = target::parse(href)?;
        if target.scheme != page.scheme || target.authority != page.authority {
            return None;
        }
        target.path
    } else if let Some(absolute) = href.strip_prefix('/') {
        format!("/{absolute}")
    } else {
        format!("{}{href}", page.path)
    };
    // Nothing that climbs, and nothing that pretends not to.
    if path.contains("/../") || path.ends_with("/..") || path.contains("/./") {
        return None;
    }
    let child = path.strip_prefix(&page.path)?;
    if child.is_empty() {
        return None;
    }
    // Exactly one level: a trailing slash is a directory, an inner one is too deep.
    let inner = child.trim_end_matches('/');
    if inner.is_empty() || inner.contains('/') {
        return None;
    }
    Some(child.to_owned())
}

/// The byte count nginx and lighttpd print at the end of a listing row.
///
/// Apache prints `1.2K` and Caddy prints `1.2 MiB`; both answer `None` here rather than a
/// guess. A missing size costs nothing — the transfer learns the real one from the server —
/// whereas a wrong one would be shown to somebody as a fact.
fn trailing_size(row: &str) -> Option<u64> {
    let token = row.split_whitespace().last()?;
    if token.len() > 19 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Entry, entries, is_index};
    use crate::target::parse;

    const NGINX: &str = r#"<html><head><title>Index of /pub/</title></head>
<body><h1>Index of /pub/</h1><hr><pre><a href="../">../</a>
<a href="Season%201/">Season 1/</a>                     27-Mar-2024 15:24    -
<a href="disc.iso">disc.iso</a>                         27-Mar-2024 15:24    1048576
</pre><hr></body></html>"#;

    const APACHE: &str = r#"<html><head><title>Index of /pub</title></head><body>
<h1>Index of /pub</h1><table><tr><th>Name</th></tr>
<tr><td><a href="/">Parent Directory</a></td></tr>
<tr><td><a href="notes.txt">notes.txt</a></td><td>2024-03-27 15:24</td><td>1.2K</td></tr>
<tr><td><a href="sub/">sub/</a></td><td>2024-03-27 15:24</td><td>  - </td></tr>
<tr><td><a href="?C=M;O=A">Last modified</a></td></tr>
</table></body></html>"#;

    fn names(html: &str, at: &str) -> Vec<Entry> {
        entries(&parse(at).expect("page"), html)
    }

    /// An nginx listing becomes its files and its subdirectory, with the byte count nginx
    /// actually prints and without the link back up.
    #[test]
    fn an_nginx_listing_becomes_its_entries() {
        let found = names(NGINX, "https://files.example.org/pub/");
        assert_eq!(
            found,
            vec![
                Entry::Directory {
                    url: "https://files.example.org/pub/Season%201/".to_owned(),
                    name: "Season 1".to_owned(),
                },
                Entry::File {
                    url: "https://files.example.org/pub/disc.iso".to_owned(),
                    name: "disc.iso".to_owned(),
                    size: Some(1_048_576),
                },
            ]
        );
    }

    /// Apache writes the parent as an absolute path and its sort links as queries; neither
    /// is an entry, and its human-readable size is not guessed at.
    #[test]
    fn an_apache_listing_drops_the_parent_and_the_sort_links() {
        let found = names(APACHE, "https://files.example.org/pub/");
        assert_eq!(
            found,
            vec![
                Entry::File {
                    url: "https://files.example.org/pub/notes.txt".to_owned(),
                    name: "notes.txt".to_owned(),
                    size: None,
                },
                Entry::Directory {
                    url: "https://files.example.org/pub/sub/".to_owned(),
                    name: "sub".to_owned(),
                },
            ]
        );
    }

    /// A page a stranger controls cannot point the crawl anywhere but under the address it
    /// was given: not at another host, not further up, and not three levels down.
    #[test]
    fn a_link_that_leaves_the_crawled_path_is_not_an_entry() {
        let html = r##"
            <a href="https://evil.invalid/x">elsewhere</a>
            <a href="http://files.example.org/pub/plain">another scheme</a>
            <a href="/etc/passwd">up and away</a>
            <a href="../../root/">climbing</a>
            <a href="deep/deeper/file.bin">too deep</a>
            <a href="//files.example.org/pub/x">protocol relative</a>
            <a href="#top">a fragment</a>
            <a href="good.bin">the only entry</a>"##;
        let found = names(html, "https://files.example.org/pub/");
        assert_eq!(
            found,
            vec![Entry::File {
                url: "https://files.example.org/pub/good.bin".to_owned(),
                name: "good.bin".to_owned(),
                size: None,
            }]
        );
    }

    /// The four servers are recognised, and a page that is simply a web page is not.
    #[test]
    fn a_listing_is_told_apart_from_a_page_that_merely_has_links() {
        assert!(is_index(NGINX), "nginx writes the parent as ../");
        assert!(is_index(APACHE), "Apache writes Index of and a parent link");
        assert!(is_index(r#"<a href="../">Go up</a>"#), "caddy and lighttpd");
        assert!(is_index("<title>Directory listing for /pub/</title>"));
        assert!(!is_index(
            r#"<html><body><h1>Welcome</h1><a href="shop.html">Shop</a></body></html>"#
        ));
    }
}
