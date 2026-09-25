//! Plain text link lists, the simplest container there is (pyLoad's `containers/TXT.py`).
//!
//! One link per line. A line in square brackets opens a package and names it, so a single file
//! can carry several; lines starting with `;` or `#` are comments. Anything that is not a URL
//! is dropped rather than refused, because these files are written by hand and a stray note in
//! one is not a reason to reject the rest.

use crate::{
    dlc::{DlcDocument, DlcFile, DlcPackage},
    links::extract_urls,
};

/// A text list this size is already implausible; the cap keeps a stray file from being read
/// into memory whole.
pub const MAX_TEXT_LIST_BYTES: usize = 4 * 1024 * 1024;

/// Reads a link list into the same shape every other container produces.
///
/// Links appearing before any `[Name]` line go into an unnamed package, which the import names
/// after the file — the same thing a DLC without a package name gets.
#[must_use]
pub fn parse_link_list(input: &[u8]) -> DlcDocument {
    // The cap was declared and never applied: every sibling container checks its own
    // (`nzb`, `dlc`, `rsdf`), so an oversized `.txt` from an upload or a hotfolder was the
    // one intake path that decoded the whole file into a `String` regardless of size.
    let input = &input[..input.len().min(MAX_TEXT_LIST_BYTES)];
    let text = String::from_utf8_lossy(strip_bom(input));
    let mut packages: Vec<DlcPackage> = Vec::new();
    let mut current: Option<DlcPackage> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = package_heading(line) {
            if let Some(package) = current.take() {
                packages.push(package);
            }
            current = Some(empty_package(Some(name)));
            continue;
        }
        // Reuses the pasted-link parser, so a text file and a paste behave identically —
        // including the host aliases and the URL canonicalisation that go with it.
        for url in extract_urls(line) {
            let package = current.get_or_insert_with(|| empty_package(None));
            package.files.push(DlcFile {
                url,
                file_name: None,
                size: None,
            });
        }
    }
    if let Some(package) = current.take() {
        packages.push(package);
    }
    packages.retain(|package| !package.files.is_empty());
    DlcDocument { packages }
}

/// `[Some Name]` on a line of its own opens a package.
fn package_heading(line: &str) -> Option<&str> {
    let name = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!name.is_empty()).then_some(name)
}

fn empty_package(name: Option<&str>) -> DlcPackage {
    DlcPackage {
        name: name.map(str::to_owned),
        password: None,
        comment: None,
        files: Vec::new(),
    }
}

/// Editors on Windows write one, and it would otherwise become part of the first line.
fn strip_bom(input: &[u8]) -> &[u8] {
    input.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(input)
}

#[cfg(test)]
mod tests {
    use super::parse_link_list;

    #[test]
    fn every_link_lands_in_the_fallback_package() {
        let document = parse_link_list(b"https://example.test/a.bin\nhttps://example.test/b.bin\n");

        assert_eq!(document.packages.len(), 1);
        assert_eq!(
            document.packages[0].name, None,
            "named after the file on import"
        );
        assert_eq!(document.packages[0].files.len(), 2);
    }

    #[test]
    fn a_heading_opens_a_package_and_names_it() {
        let document = parse_link_list(
            b"[Season 1]\nhttps://example.test/e01.bin\n[Season 2]\nhttps://example.test/e02.bin\n",
        );

        let names: Vec<&str> = document
            .packages
            .iter()
            .filter_map(|package| package.name.as_deref())
            .collect();
        assert_eq!(names, ["Season 1", "Season 2"]);
        assert!(
            document
                .packages
                .iter()
                .all(|package| package.files.len() == 1)
        );
    }

    #[test]
    fn comments_blank_lines_and_a_byte_order_mark_are_ignored() {
        let document = parse_link_list(
            "\u{feff}; written by hand\n\n# another comment\nhttps://example.test/a.bin\n"
                .as_bytes(),
        );

        assert_eq!(document.packages.len(), 1, "{document:?}");
        assert_eq!(document.packages[0].files.len(), 1);
    }

    #[test]
    fn a_heading_with_nothing_under_it_produces_no_package() {
        let document = parse_link_list(b"[Empty]\nnot a link at all\n");

        assert!(document.packages.is_empty(), "{document:?}");
    }

    #[test]
    fn a_note_beside_a_link_does_not_lose_the_link() {
        let document = parse_link_list(b"see https://example.test/a.bin for the rest\n");

        assert_eq!(document.packages[0].files.len(), 1);
    }
}
