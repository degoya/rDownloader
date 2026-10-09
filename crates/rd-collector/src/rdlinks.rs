//! `.rdlinks`, rDownloader's own link list (RD-1210-01).
//!
//! A package leaves the application as what makes its links: the addresses a person gave, the
//! package's name, archive password, category name and comment, and per link the file name,
//! size, checksum and mirror group. Never a plugin, a plugin version, an account, a cookie or a
//! token — the point of the file is that the links are assigned to a host again when they come
//! back, and resolved by whatever plugin is installed then. A DLC cannot be written without a
//! registered `dlcrypt` identity, so this is the format rDownloader writes; `.crawljob` (see
//! [`crate::write_crawljob`]) is the one JDownloader reads.
//!
//! A Usenet download or an indexer hit has no address that works anywhere else — an indexer's
//! needs its API key, which never goes into a file — so a package carries those as the NZB
//! document itself (`nzbs`, RD-1220-02): its name and the XML, written out from its files, groups
//! and articles. The member is additive in `rdownloader-links/1`: there is no installed base to
//! keep a reader of the older shape working for (AGENTS.md), and a sealed file seals the NZBs
//! with the links because they are part of the same packages document.
//!
//! The document is JSON with a `format` marker. It is either readable (`packages`) or sealed
//! (`encryption`); the sealing itself — Argon2id and XChaCha20-Poly1305, the settings backup's
//! primitives — lives with the service, which holds the key derivation. This module reads and
//! writes the outer shape and checks every document it hands on, so a file somebody edited into
//! something else is refused here rather than half imported.

use anyhow::{Context, Result, bail};
use rd_core::ExpectedChecksum;
use serde::{Deserialize, Serialize};
use url::Url;

/// The value of `format` in every document this build writes and the only one it reads.
pub const RDLINKS_FORMAT: &str = "rdownloader-links/1";
/// The largest `.rdlinks` file read or written: 48 MiB, the largest file an import's JSON body
/// carries (`rd_api_core::container_upload::MAX_JSON_CONTAINER_BYTES`), so whatever the export
/// writes comes back through either body. Links alone stay far below it; the embedded NZBs are
/// what needs the room (RD-1220-02).
pub const MAX_RDLINKS_BYTES: usize = 48 * 1024 * 1024;
/// The most links and NZBs one document carries together, on the way out and on the way back in.
pub const MAX_RDLINKS_LINKS: usize = 2_000;
/// The longest name, password, category, comment, file name or mirror group kept.
const MAX_TEXT_CHARS: usize = 1_024;
/// The longest address kept.
const MAX_URL_CHARS: usize = 8_192;
/// The schemes a link may carry; anything else is not a link this application downloads.
const SCHEMES: [&str; 6] = ["http", "https", "ftp", "ftps", "sftp", "magnet"];

/// The packages of one document.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinksDocument {
    pub packages: Vec<LinksPackage>,
}

/// One package and its links.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinksPackage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The archive password, readable in an unsealed document like everywhere else it is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The category by name, never by id: an id means nothing on another installation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// May be empty when the package carries only NZBs.
    pub links: Vec<LinksEntry>,
    /// The NZB documents of the package's Usenet downloads and indexer hits (RD-1220-02).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nzbs: Vec<LinksNzb>,
}

/// One NZB document carried whole: what an indexer hit or a Usenet download is made of, without
/// the address and the key it was fetched with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinksNzb {
    /// The release name, without `.nzb`.
    pub name: String,
    /// The NZB's XML, at most [`crate::MAX_NZB_BYTES`].
    pub content: String,
}

/// One link: the address a person gave, not the direct address a resolver once answered with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinksEntry {
    pub url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<ExpectedChecksum>,
    /// Links of one package with the same group are copies of one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_group: Option<String>,
}

/// How a sealed document's key is derived from its passphrase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinksKdf {
    pub algorithm: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    /// Base64 of the salt.
    pub salt: String,
}

/// A sealed document's `encryption` member: the packages as ciphertext, and how to open them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SealedLinks {
    pub kdf: LinksKdf,
    pub cipher: String,
    /// Base64 of the nonce.
    pub nonce: String,
    /// Base64 of the ciphertext, whose plaintext is a `{"packages": […]}` document.
    pub ciphertext: String,
}

/// What a file turned out to be.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinksFile {
    Plain(LinksDocument),
    Sealed(SealedLinks),
}

/// The outer shape, with exactly one of the two bodies.
#[derive(Deserialize, Serialize)]
struct Envelope {
    format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    packages: Option<Vec<LinksPackage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encryption: Option<SealedLinks>,
}

/// Reads a `.rdlinks` file: a checked document, or the sealed body still to be opened.
///
/// # Errors
///
/// When the file is too large, is not this format's JSON, names another format version, carries
/// both or neither body, or holds a link or a field this format does not allow.
pub fn read_links_file(input: &[u8]) -> Result<LinksFile> {
    if input.len() > MAX_RDLINKS_BYTES {
        bail!("the file exceeds the {} MiB limit", MAX_RDLINKS_BYTES >> 20);
    }
    let envelope: Envelope =
        serde_json::from_slice(input).context("the file is not an rdownloader-links document")?;
    if envelope.format != RDLINKS_FORMAT {
        bail!("the file is not an {RDLINKS_FORMAT} document");
    }
    match (envelope.packages, envelope.encryption) {
        (Some(packages), None) => {
            let document = LinksDocument { packages };
            check_document(&document)?;
            Ok(LinksFile::Plain(document))
        }
        (None, Some(sealed)) => Ok(LinksFile::Sealed(sealed)),
        _ => bail!("the document must carry either packages or an encryption member"),
    }
}

/// Reads the plaintext a sealed document opened to, with the same checks a readable one gets.
///
/// # Errors
///
/// When the plaintext is not a packages document or does not pass the checks.
pub fn read_sealed_plaintext(plaintext: &[u8]) -> Result<LinksDocument> {
    let document: LinksDocument =
        serde_json::from_slice(plaintext).context("the sealed packages are not readable")?;
    check_document(&document)?;
    Ok(document)
}

/// The readable file for `document`.
///
/// # Errors
///
/// When the document does not pass the checks a reader applies: what is written is readable.
pub fn write_links_file(document: &LinksDocument) -> Result<Vec<u8>> {
    check_document(document)?;
    let envelope = Envelope {
        format: RDLINKS_FORMAT.to_owned(),
        packages: Some(document.packages.clone()),
        encryption: None,
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

/// The plaintext a sealed file's ciphertext is made of.
///
/// # Errors
///
/// When the document does not pass the checks a reader applies.
pub fn sealed_plaintext(document: &LinksDocument) -> Result<Vec<u8>> {
    check_document(document)?;
    Ok(serde_json::to_vec(document)?)
}

/// The sealed file around an already sealed body.
///
/// # Errors
///
/// Only when JSON serialisation fails, which plain strings and numbers do not.
pub fn write_sealed_file(sealed: &SealedLinks) -> Result<Vec<u8>> {
    let envelope = Envelope {
        format: RDLINKS_FORMAT.to_owned(),
        packages: None,
        encryption: Some(sealed.clone()),
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

/// How many links a document carries.
#[must_use]
pub fn link_count(document: &LinksDocument) -> usize {
    document
        .packages
        .iter()
        .map(|package| package.links.len())
        .sum()
}

/// How many NZB documents a document carries.
#[must_use]
pub fn nzb_count(document: &LinksDocument) -> usize {
    document
        .packages
        .iter()
        .map(|package| package.nzbs.len())
        .sum()
}

/// Whether this format carries a link with this address's scheme.
#[must_use]
pub fn carries_scheme(url: &Url) -> bool {
    SCHEMES.contains(&url.scheme())
}

/// Everything a document must hold to be written or read: at least one link or NZB, at most
/// [`MAX_RDLINKS_LINKS`] of both, only schemes the application downloads, NZBs with a name and of
/// at most [`crate::MAX_NZB_BYTES`], and no field of a length no person writes. Whether an NZB's
/// XML is an NZB is the importer's question: it parses every one before it creates anything.
fn check_document(document: &LinksDocument) -> Result<()> {
    let total = link_count(document) + nzb_count(document);
    if total == 0 {
        bail!("the document holds no links");
    }
    if total > MAX_RDLINKS_LINKS {
        bail!("the document holds {total} links, more than the {MAX_RDLINKS_LINKS} allowed");
    }
    for package in &document.packages {
        for text in [
            &package.name,
            &package.password,
            &package.category,
            &package.comment,
        ] {
            check_text(text.as_deref())?;
        }
        for nzb in &package.nzbs {
            if nzb.name.trim().is_empty() {
                bail!("an NZB has no name");
            }
            check_text(Some(&nzb.name))?;
            if nzb.content.len() > crate::MAX_NZB_BYTES {
                bail!(
                    "an NZB exceeds the {} MiB limit",
                    crate::MAX_NZB_BYTES >> 20
                );
            }
        }
        for link in &package.links {
            if !carries_scheme(&link.url) {
                bail!(
                    "a link uses the scheme {}, which is not allowed",
                    link.url.scheme()
                );
            }
            if link.url.as_str().len() > MAX_URL_CHARS {
                bail!("a link is longer than {MAX_URL_CHARS} characters");
            }
            check_text(link.file_name.as_deref())?;
            check_text(link.mirror_group.as_deref())?;
            if let Some(checksum) = &link.checksum
                && (checksum.value.is_empty()
                    || checksum.value.len() > 256
                    || !checksum
                        .value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric()))
            {
                bail!("a checksum is not a hash value");
            }
        }
    }
    Ok(())
}

fn check_text(text: Option<&str>) -> Result<()> {
    match text {
        Some(text) if text.chars().count() > MAX_TEXT_CHARS => {
            bail!("a field is longer than {MAX_TEXT_CHARS} characters")
        }
        Some(text)
            if text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t')) =>
        {
            bail!("a field contains control characters")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "rdlinks_tests.rs"]
mod tests;
