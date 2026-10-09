//! Writing a `.crawljob`, so an exported list also lands in JDownloader (RD-1210-01).
//!
//! JDownloader's folder watch reads `key=value` lines, one blank-line-separated block per job.
//! A block here is one package: `text` with its addresses, `packageName`, the archive password
//! as `extractPasswords` and `autoStart=FALSE`, so the links arrive in JDownloader's LinkGrabber
//! as proposals and nothing starts on its own — the same terms a link arriving here is held to.
//! Nothing else: no folder, no file names, no plugin, and no encryption, which the format has
//! none of. `plugins/crawljob-intake` reads the same file back, and its golden-file test keeps
//! the two in step.

use crate::rdlinks::LinksDocument;

/// A written `.crawljob` and how many links it could not carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Crawljob {
    pub text: String,
    /// Links whose scheme a crawljob does not carry (anything but http and https).
    pub skipped: usize,
}

/// The `.crawljob` for `document`: one block per package that keeps at least one link.
#[must_use]
pub fn write_crawljob(document: &LinksDocument) -> Crawljob {
    let mut blocks = Vec::new();
    let mut skipped = 0;
    for package in &document.packages {
        let urls: Vec<&str> = package
            .links
            .iter()
            .filter(|link| matches!(link.url.scheme(), "http" | "https"))
            .map(|link| link.url.as_str())
            .collect();
        skipped += package.links.len() - urls.len();
        if urls.is_empty() {
            continue;
        }
        let mut block = format!("text={}\n", urls.join(" "));
        if let Some(name) = one_line(package.name.as_deref()) {
            block.push_str(&format!("packageName={name}\n"));
        }
        if let Some(password) = one_line(package.password.as_deref()) {
            // A JSON array of strings is how JDownloader spells the list; serialising one string
            // cannot fail.
            let list = serde_json::to_string(&[password]).unwrap_or_default();
            block.push_str(&format!("extractPasswords={list}\n"));
        }
        block.push_str("autoStart=FALSE\n");
        blocks.push(block);
    }
    Crawljob {
        text: blocks.join("\n"),
        skipped,
    }
}

/// A value on one line: a line break would end the key, so control characters become spaces.
fn one_line(value: Option<&str>) -> Option<String> {
    let value: String = value?
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::write_crawljob;
    use crate::rdlinks::{LinksDocument, LinksEntry, LinksPackage};

    fn link(address: &str) -> LinksEntry {
        LinksEntry {
            url: address.parse().expect("address"),
            file_name: Some("ignored.bin".to_owned()),
            size: Some(1),
            checksum: None,
            mirror_group: None,
        }
    }

    /// The document the golden file was written from.
    fn golden_document() -> LinksDocument {
        LinksDocument {
            packages: vec![
                LinksPackage {
                    name: Some("Holiday 2026".to_owned()),
                    password: Some("se\"cret".to_owned()),
                    category: Some("Videos".to_owned()),
                    comment: Some("not carried".to_owned()),
                    links: vec![
                        link("https://ddownload.com/abc123/holiday.part1.rar"),
                        link("https://ddownload.com/def456/holiday.part2.rar"),
                        link("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"),
                    ],
                },
                LinksPackage {
                    name: Some("Second\nline".to_owned()),
                    password: None,
                    category: None,
                    comment: None,
                    links: vec![link("http://example.com/file.bin")],
                },
                LinksPackage {
                    name: Some("Only a torrent".to_owned()),
                    password: None,
                    category: None,
                    comment: None,
                    links: vec![link(
                        "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef",
                    )],
                },
            ],
        }
    }

    /// What JDownloader and `plugins/crawljob-intake` read (RD-1210-01): the writer's output is
    /// the golden file byte for byte, and the plugin's own test parses that same file.
    #[test]
    fn the_export_is_the_golden_file_the_crawljob_parser_reads() {
        let root = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
        );
        let golden =
            root.join("../../plugins/crawljob-intake/tests/golden/rdownloader-export.crawljob");
        let expected = std::fs::read_to_string(&golden).expect("golden file");
        let written = write_crawljob(&golden_document());
        assert_eq!(written.text, expected);
        // Two magnet links have no place in a crawljob; the package holding only one is left out.
        assert_eq!(written.skipped, 2);
    }

    #[test]
    fn nothing_but_addresses_name_and_password_leaves() {
        let written = write_crawljob(&golden_document()).text;
        assert!(!written.contains("ignored.bin"), "no file names");
        assert!(!written.contains("Videos"), "no category");
        assert!(!written.contains("not carried"), "no comment");
        assert!(!written.contains("downloadFolder"), "no folder");
    }
}
