//! Deciding whether an address is a Google Drive *file*, and which one.
//!
//! Answered from the address alone, because `match-url` is asked of every link a person pastes
//! and must reach nothing. What it must not do is over-claim: a folder address belongs to the
//! sibling crawler, and a resolver that swallowed it would turn a folder of two hundred files
//! into one refusal.

use google_drive_common::address::{google_host, query_value, segments, split, valid_id};

/// A Drive address this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    /// The Drive file id, opaque to this plugin.
    pub id: String,
    /// The export format the address asked for, when it named one — Google's own export
    /// addresses spell it `?format=docx`. `None` means "whatever this document type exports as
    /// by default".
    pub format: Option<String>,
}

/// An export format as Google spells one: a short lowercase word.
fn valid_format(format: &str) -> bool {
    !format.is_empty() && format.len() <= 8 && format.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// The Workspace editors, each of which spells a document address `/<editor>/d/<id>`.
const EDITORS: [&str; 5] = [
    "document",
    "spreadsheets",
    "presentation",
    "drawings",
    "forms",
];

/// Reads an address, returning the file this plugin would fetch — or `None`, which is what
/// `match-url` answers for everything that is not a Google Drive *file* address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    let (host, path) = split(url)?;
    let host = google_host(host)?;
    let route = path.split('?').next()?;
    let parts = segments(route);
    let format = query_value(path, "format")
        .or_else(|| query_value(path, "exportFormat"))
        .filter(|format| valid_format(format))
        .map(str::to_owned);
    let id = match (host, parts.as_slice()) {
        // `https://drive.google.com/file/d/<id>/view`
        ("drive.google.com", ["file", "d", id, ..]) => (*id).to_owned(),
        // `https://drive.google.com/open?id=<id>` and the legacy `/uc?id=<id>&export=download`.
        // Deliberately *not* `/drive/folders/<id>`: a folder is the crawler's address, and a
        // resolver that claimed it would answer one refusal for a folder of many files.
        ("drive.google.com", ["open"] | ["uc"]) => query_value(path, "id")?.to_owned(),
        // Where a public share ends up once Google has redirected it.
        ("drive.usercontent.google.com", ["download"]) => query_value(path, "id")?.to_owned(),
        // `https://docs.google.com/document/d/<id>/edit`
        ("docs.google.com", [editor, "d", id, ..]) if EDITORS.contains(editor) => (*id).to_owned(),
        // The canonical per-file API address.
        ("www.googleapis.com", ["drive", "v3", "files", id, ..]) => (*id).to_owned(),
        _ => return None,
    };
    valid_id(&id).then_some(Target { id, format })
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    fn plain(id: &str) -> Option<Target> {
        Some(Target {
            id: id.to_owned(),
            format: None,
        })
    }

    #[test]
    fn every_spelling_of_a_file_address_is_claimed() {
        let id = "1A2b3C4d5E6f7G8h9I0j";
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/file/d/{id}/view?usp=sharing"
            )),
            plain(id)
        );
        assert_eq!(
            claim(&format!("https://drive.google.com/open?id={id}")),
            plain(id)
        );
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/uc?id={id}&export=download"
            )),
            plain(id)
        );
        assert_eq!(
            claim(&format!(
                "https://drive.usercontent.google.com/download?id={id}&export=download"
            )),
            plain(id)
        );
        assert_eq!(
            claim(&format!(
                "https://docs.google.com/spreadsheets/d/{id}/edit#gid=0"
            )),
            plain(id)
        );
        // The `/u/<n>` prefix Google puts in front of nearly every link of its own.
        assert_eq!(
            claim(&format!(
                "https://docs.google.com/u/2/presentation/d/{id}/edit"
            )),
            plain(id)
        );
        // And the canonical address the sibling crawler hands back.
        assert_eq!(
            claim(&google_drive_common::address::file_address(id)),
            plain(id)
        );
        assert_eq!(
            claim(&format!(
                "https://www.googleapis.com/drive/v3/files/{id}?alt=media"
            )),
            plain(id)
        );
    }

    /// The address a person asked an export format for keeps it, so the name and the extension
    /// they will get are decided before anything is queued.
    #[test]
    fn an_address_that_names_an_export_format_carries_it_through() {
        let id = "1A2b3C4d5E6f7G8h9I0j";
        assert_eq!(
            claim(&format!(
                "https://docs.google.com/document/d/{id}/export?format=pdf"
            )),
            Some(Target {
                id: id.to_owned(),
                format: Some("pdf".to_owned()),
            })
        );
        // A "format" that is not one is dropped rather than sent on: it would otherwise reach a
        // request path.
        assert_eq!(
            claim(&format!(
                "https://docs.google.com/document/d/{id}/export?format=../evil"
            )),
            plain(id)
        );
    }

    /// A folder belongs to the sibling crawler. This is the over-claim that would turn a folder
    /// of two hundred files into one refusal.
    #[test]
    fn a_folder_address_is_left_to_the_crawler() {
        let id = "1A2b3C4d5E6f7G8h9I0j";
        assert_eq!(
            claim(&format!("https://drive.google.com/drive/folders/{id}")),
            None
        );
        assert_eq!(
            claim(&format!("https://drive.google.com/drive/u/0/folders/{id}")),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(
            claim("https://drive.google.com.evil.test/file/d/abc/view"),
            None
        );
        assert_eq!(claim("https://x@drive.google.com/file/d/abc/view"), None);
        assert_eq!(claim("ftp://drive.google.com/file/d/abc/view"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://drive.google.com/"), None);
        assert_eq!(claim("https://www.googleapis.com/oauth2/v1/userinfo"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(
            claim("https://drive.google.com/file/d/../../etc/view"),
            None
        );
        assert_eq!(claim("https://drive.google.com/file/d/a%2Fb/view"), None);
        assert_eq!(claim("https://drive.google.com/open?id="), None);
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/file/d/{}/view",
                "x".repeat(129)
            )),
            None
        );
    }
}
