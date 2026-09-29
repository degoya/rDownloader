//! A scaffold intake parser. It compiles, packages and passes conformance as it is.
//!
//! It finds `https://files.example.com/d/<id>` links anywhere in pasted text — in prose, in
//! brackets, behind a tracking query — and groups them under a `[Package name]` line when the
//! text has one. Replace [`PREFIX`] and the reading with your format's.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - **You propose; the application decides.** Everything you return goes through the
//!   LinkGrabber review, the domain blocklist and the routing rules, exactly as a pasted link
//!   does, and a package hint is a suggestion, not a package.
//! - **A rewrite from `normalize` is discarded if it changes the host or the scheme.** A
//!   normalizer tidies an address; it cannot redirect one.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// Every link this parser proposes starts with this.
pub const PREFIX: &str = "https://files.example.com/d/";

/// One link found in the text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub url: String,
    /// The `[Package name]` line the link stood under, if any.
    pub package: Option<String>,
}

/// Whether the text holds a single link this parser reads. Asked before `parse`, and for every
/// paste in the application, so it is cheap and claims narrowly.
#[must_use]
pub fn claims(input: &str) -> bool {
    input.lines().any(|line| links(line).next().is_some())
}

/// The links in `input`, in order, each once, in canonical form.
#[must_use]
pub fn parse(input: &str) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut package = None;
    for line in input.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            package = Some(name.trim().to_owned()).filter(|name| !name.is_empty());
            continue;
        }
        for link in links(line) {
            let url = normalize(link).unwrap_or_else(|| link.to_owned());
            if found.iter().all(|candidate| candidate.url != url) {
                found.push(Candidate {
                    url,
                    package: package.clone(),
                });
            }
        }
    }
    found
}

/// The canonical form of a link — `PREFIX` and the id, without query, fragment or trailing
/// slash — or `None` when it already is canonical or is not this parser's.
///
/// `None` for "unchanged" rather than the same string back, so the caller can tell the two
/// apart.
#[must_use]
pub fn normalize(url: &str) -> Option<String> {
    let id = url
        .strip_prefix(PREFIX)?
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default();
    if id.is_empty() {
        return None;
    }
    let canonical = format!("{PREFIX}{id}");
    (canonical != url).then_some(canonical)
}

/// The words of a line that are links of ours, with the punctuation prose puts around them.
fn links(line: &str) -> impl Iterator<Item = &str> {
    line.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | ';'))
        })
        .filter(|word| word.len() > PREFIX.len() && word.starts_with(PREFIX))
}

#[cfg(test)]
mod tests {
    use super::{Candidate, claims, normalize, parse};

    #[test]
    fn only_text_with_one_of_our_links_is_claimed() {
        assert!(claims("see https://files.example.com/d/AbC for it"));
        assert!(!claims("https://elsewhere.example.org/d/AbC"));
        assert!(!claims("https://files.example.com/d/"));
    }

    #[test]
    fn links_are_found_in_prose_grouped_and_listed_once() {
        let input = "[Holiday]\nPart one (https://files.example.com/d/A1?ref=feed), part two:\n\
                     <https://files.example.com/d/B2>\n\nhttps://files.example.com/d/A1/\n";
        let candidate = |url: &str| Candidate {
            url: url.to_owned(),
            package: Some("Holiday".to_owned()),
        };
        assert_eq!(
            parse(input),
            vec![
                candidate("https://files.example.com/d/A1"),
                candidate("https://files.example.com/d/B2"),
            ]
        );
    }

    #[test]
    fn normalizing_tidies_and_never_moves_a_link() {
        assert_eq!(
            normalize("https://files.example.com/d/A1/?utm_source=x"),
            Some("https://files.example.com/d/A1".to_owned())
        );
        assert_eq!(normalize("https://files.example.com/d/A1"), None);
        assert_eq!(normalize("https://elsewhere.example.org/d/A1?x"), None);
    }
}
