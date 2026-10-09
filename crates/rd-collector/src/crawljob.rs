//! Writing a `.crawljob`, so an exported list also lands in JDownloader (RD-1210-01).
//!
//! JDownloader's folder watch reads `key=value` lines, one blank-line-separated block per job.
//! A block here is one package: `text` with its addresses, `packageName`, the archive password
//! as `extractPasswords` and `autoStart=FALSE`, so the links arrive in JDownloader's LinkGrabber
//! as proposals and nothing starts on its own — the same terms a link arriving here is held to.
//! Nothing else: no folder, no file names, no plugin, and no encryption, which the format has
//! none of. `plugins/crawljob-intake` reads the same file back, and its golden-file test keeps
//! the two in step.
//!
//! [`read_crawljob`] is the host's own reader, for a `.crawljob` handed to the import dialog
//! (RD-1220-02): the plugin proposes links from pasted text, the dialog takes a file. It reads
//! what the plugin reads — `text`, `packageName`, `filename` for a single link — and the archive
//! password from `extractPasswords`, and leaves `downloadFolder`, `autoStart` and every other key
//! alone, as the plugin does: what the other application should do afterwards is not this file's
//! to decide here. The import dialog's own choices (category, queueing) apply instead.

use anyhow::{Result, bail};

use crate::{
    dlc::{DlcDocument, DlcFile, DlcPackage},
    rdlinks::LinksDocument,
};

/// The largest `.crawljob` read: a link list like a `.txt`, never payload.
pub const MAX_CRAWLJOB_BYTES: usize = crate::textlist::MAX_TEXT_LIST_BYTES;

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

/// Reads a `.crawljob` into the shape every container produces: one package per block that
/// holds at least one http(s) link. A file without one comes back empty, which the import
/// reports like every other empty container.
///
/// # Errors
///
/// Only for a file over [`MAX_CRAWLJOB_BYTES`].
pub fn read_crawljob(input: &[u8]) -> Result<DlcDocument> {
    if input.len() > MAX_CRAWLJOB_BYTES {
        bail!(
            "the crawljob exceeds the {} MiB limit",
            MAX_CRAWLJOB_BYTES >> 20
        );
    }
    let text = String::from_utf8_lossy(input.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(input));
    let mut document = DlcDocument::default();
    let mut block = Block::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            std::mem::take(&mut block).finish(&mut document);
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_lowercase().as_str() {
            "text" => block.urls.extend(links_in(value)),
            "packagename" if !value.is_empty() => block.name = Some(value.to_owned()),
            "filename" if !value.is_empty() => block.file_name = Some(value.to_owned()),
            "extractpasswords" => block.password = first_password(value),
            _ => {}
        }
    }
    block.finish(&mut document);
    Ok(document)
}

/// One block while it is read.
#[derive(Default)]
struct Block {
    urls: Vec<url::Url>,
    name: Option<String>,
    file_name: Option<String>,
    password: Option<String>,
}

impl Block {
    fn finish(self, document: &mut DlcDocument) {
        if self.urls.is_empty() {
            return;
        }
        // `filename=` names the one link of a single-link job; with several it says nothing.
        let file_name = self.file_name.filter(|_| self.urls.len() == 1);
        document.packages.push(DlcPackage {
            name: self.name,
            password: self.password,
            comment: None,
            files: self
                .urls
                .into_iter()
                .map(|url| DlcFile {
                    url,
                    file_name: file_name.clone(),
                    size: None,
                })
                .collect(),
        });
    }
}

/// The http(s) links of one `text=` value, separated by a literal `\n`, spaces, commas or
/// semicolons depending on what wrote the file — the plugin's rule.
fn links_in(value: &str) -> Vec<url::Url> {
    value
        .replace("\\n", " ")
        .replace("\\r", " ")
        .split([' ', '\t', ',', ';'])
        .map(str::trim)
        .filter(|token| token.starts_with("http://") || token.starts_with("https://"))
        .filter_map(|token| url::Url::parse(token).ok())
        .collect()
}

/// The first password of `extractPasswords`: JDownloader writes a JSON list of strings, a hand
/// may write one bare value.
fn first_password(value: &str) -> Option<String> {
    let listed = serde_json::from_str::<Vec<String>>(value)
        .ok()
        .and_then(|list| {
            list.into_iter()
                .map(|item| item.trim().to_owned())
                .find(|item| !item.is_empty())
        });
    match listed {
        Some(password) => Some(password),
        None if value.starts_with('[') => None,
        None => one_line(Some(value)),
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
    use super::{read_crawljob, write_crawljob};
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
                    nzbs: Vec::new(),
                },
                LinksPackage {
                    name: Some("Second\nline".to_owned()),
                    password: None,
                    category: None,
                    comment: None,
                    links: vec![link("http://example.com/file.bin")],
                    nzbs: Vec::new(),
                },
                LinksPackage {
                    name: Some("Only a torrent".to_owned()),
                    password: None,
                    category: None,
                    comment: None,
                    links: vec![link(
                        "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef",
                    )],
                    nzbs: Vec::new(),
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

    /// The host reads the golden file the writer produces (RD-1220-02): every block with its
    /// links, its package name and its password; `autoStart` decides nothing.
    #[test]
    fn the_golden_file_is_read_back_by_the_host() {
        let root = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
        );
        let golden =
            root.join("../../plugins/crawljob-intake/tests/golden/rdownloader-export.crawljob");
        let read = read_crawljob(&std::fs::read(golden).expect("golden file")).expect("read");
        let summary: Vec<(Option<&str>, Option<&str>, Vec<&str>)> = read
            .packages
            .iter()
            .map(|package| {
                (
                    package.name.as_deref(),
                    package.password.as_deref(),
                    package.files.iter().map(|file| file.url.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    Some("Holiday 2026"),
                    Some("se\"cret"),
                    vec![
                        "https://ddownload.com/abc123/holiday.part1.rar",
                        "https://ddownload.com/def456/holiday.part2.rar",
                    ],
                ),
                (
                    Some("Second line"),
                    None,
                    vec!["http://example.com/file.bin"]
                ),
            ]
        );
    }

    #[test]
    fn a_folder_or_a_start_decides_nothing_and_only_web_links_are_read() {
        let text = "text=https://example.com/one.bin\\nftp://example.com/two.bin\n\
                    filename=One.bin\ndownloadFolder=/etc\nautoStart=TRUE\n\
                    extractPasswords=plain secret\n\n\
                    text=file:///etc/passwd\npackageName=Nothing\n";
        let read = read_crawljob(text.as_bytes()).expect("read");
        assert_eq!(
            read.packages.len(),
            1,
            "a block without a web link is dropped"
        );
        let package = &read.packages[0];
        assert_eq!(package.password.as_deref(), Some("plain secret"));
        assert_eq!(package.files.len(), 1);
        assert_eq!(package.files[0].file_name.as_deref(), Some("One.bin"));
        assert!(read_crawljob(&vec![b' '; super::MAX_CRAWLJOB_BYTES + 1]).is_err());
        assert!(
            read_crawljob(b"autoStart=TRUE\n")
                .expect("read")
                .packages
                .is_empty()
        );
    }
}
