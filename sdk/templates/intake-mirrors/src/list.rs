//! Reading a mirror list.
//!
//! The format is this scaffold's own, small enough to read at a glance: a first line
//! `# mirror-list`, then one file per line — its sources in order of preference, each optionally
//! followed by `@` and a two-letter country code, then optional `size=` and `sha-256=` fields.
//!
//! ```text
//! # mirror-list
//! https://eu.example.com/disk.iso @de https://us.example.net/disk.iso @us size=1048576 sha-256=…
//! ```
//!
//! Plain Rust with no dependencies, so it is unit-tested on the host target as it is. Replace
//! it with your format's reader and keep what the guest needs from it: a list of [`File`]s.

/// The first line of every list this plugin claims.
pub const HEADER: &str = "# mirror-list";

/// The most sources the host accepts for one file. More would be refused, so the rest of a
/// long line is left out rather than costing the whole set.
pub const MAX_SOURCES: usize = 32;

/// One address of a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Source {
    pub url: String,
    /// ISO 3166-1 alpha-2, lower case, when the list gave one.
    pub location: Option<String>,
}

/// One file and every place it can be fetched from, most preferred first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File {
    pub name: Option<String>,
    pub size: Option<u64>,
    /// Lower-case hex SHA-256 of the whole file.
    pub sha256: Option<String>,
    pub sources: Vec<Source>,
}

/// Whether `input` is a list this plugin reads. Decided from the first line alone, so a paste
/// that is not a mirror list costs nothing.
#[must_use]
pub fn claims(input: &str) -> bool {
    input.lines().next().map(str::trim) == Some(HEADER)
}

/// The files in `input`, one per line that names at least one source. Empty when `input` is
/// not a mirror list.
#[must_use]
pub fn files(input: &str) -> Vec<File> {
    if !claims(input) {
        return Vec::new();
    }
    input.lines().skip(1).filter_map(file).collect()
}

fn file(line: &str) -> Option<File> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut file = File {
        name: None,
        size: None,
        sha256: None,
        sources: Vec::new(),
    };
    for token in line.split_whitespace() {
        if token.starts_with("https://") {
            if file.sources.len() < MAX_SOURCES {
                file.sources.push(Source {
                    url: token.to_owned(),
                    location: None,
                });
            }
        } else if let Some(country) = token.strip_prefix('@') {
            // A location belongs to the source right before it, and only a well-formed one is
            // stated: the host refuses what is not a country code.
            let well_formed =
                country.len() == 2 && country.bytes().all(|b| b.is_ascii_alphabetic());
            if let (true, Some(source)) = (well_formed, file.sources.last_mut()) {
                source.location = Some(country.to_ascii_lowercase());
            }
        } else if let Some(size) = token.strip_prefix("size=") {
            file.size = size.parse().ok();
        } else if let Some(hex) = token.strip_prefix("sha-256=") {
            // A hash that is not 64 hex digits is left out rather than passed on: stated, it
            // would be refused, and a wrong one would fail every source's verification.
            if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                file.sha256 = Some(hex.to_ascii_lowercase());
            }
        }
        // Anything else is a field this reader does not know, and it is skipped.
    }
    let primary = file.sources.first()?;
    file.name = file_name(&primary.url);
    Some(file)
}

/// The last path segment of an address, without its query.
fn file_name(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let (_, rest) = path.split_once("://")?;
    let (_, name) = rest.rsplit_once('/')?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{File, MAX_SOURCES, Source, claims, files};

    const HASH: &str = "9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08";

    #[test]
    fn only_a_list_with_its_header_is_claimed() {
        assert!(claims("# mirror-list\nhttps://a.example.com/x"));
        assert!(!claims("https://a.example.com/x\n# mirror-list"));
        assert!(!claims(""));
        assert!(files("https://a.example.com/x.iso").is_empty());
    }

    #[test]
    fn a_line_is_one_file_with_its_sources_in_order() {
        let input = format!(
            "# mirror-list\n\nhttps://eu.example.com/disk.iso?x=1 @DE https://us.example.net/disk.iso @us size=1048576 sha-256={HASH}\n"
        );
        assert_eq!(
            files(&input),
            vec![File {
                name: Some("disk.iso".to_owned()),
                size: Some(1_048_576),
                sha256: Some(HASH.to_ascii_lowercase()),
                sources: vec![
                    Source {
                        url: "https://eu.example.com/disk.iso?x=1".to_owned(),
                        location: Some("de".to_owned()),
                    },
                    Source {
                        url: "https://us.example.net/disk.iso".to_owned(),
                        location: Some("us".to_owned()),
                    },
                ],
            }]
        );
    }

    #[test]
    fn what_the_host_would_refuse_is_left_out_and_not_passed_on() {
        let input = "# mirror-list\nhttp://plain.example.com/a @xyz https://a.example.com/a @1 sha-256=abc\n# a comment\nsize=5\n";
        let found = files(input);
        assert_eq!(found.len(), 1, "a line without an https source is no file");
        assert_eq!(found[0].sources.len(), 1);
        assert_eq!(found[0].sources[0].location, None);
        assert_eq!(found[0].sha256, None);
    }

    #[test]
    fn a_long_line_is_cut_at_the_hosts_limit() {
        let sources: Vec<String> = (0..40)
            .map(|index| format!("https://m{index}.example.com/f.bin"))
            .collect();
        let input = format!("# mirror-list\n{}", sources.join(" "));
        assert_eq!(files(&input)[0].sources.len(), MAX_SOURCES);
    }
}
