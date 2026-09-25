//! Deciding whether an address is a Google Drive *folder*.
//!
//! The mirror image of the sibling resolver's decision, and the reason both are narrow:
//! `claims-url` is asked of every link a person pastes, and a crawler that claimed a file
//! address would fetch a listing for every download in the queue.

use google_drive_common::address::{google_host, query_value, segments, split, valid_id};

/// A folder this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    /// The Drive folder id. A shared drive's id is one of these too: its root folder *is* the
    /// drive, which is why a shared-drive address needs no case of its own.
    pub id: String,
}

/// Reads an address, returning the folder this plugin would list — or `None`, which is what
/// `claims-url` answers for everything that is not a Google Drive folder address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    let (host, path) = split(url)?;
    if google_host(host)? != "drive.google.com" {
        return None;
    }
    let route = path.split('?').next()?;
    let parts = segments(route);
    let id = match parts.as_slice() {
        // `https://drive.google.com/drive/folders/<id>`, and the same with Google's own
        // `/u/<n>/` prefix or its `mobile/` variant.
        ["drive", "folders", id, ..] => (*id).to_owned(),
        ["drive", "u", _, "folders", id, ..] => (*id).to_owned(),
        ["drive", "mobile", "folders", id, ..] => (*id).to_owned(),
        // The address a Shared Drive's own page carries.
        ["drive", "u", _, "shared-drives", id, ..] | ["drive", "shared-drives", id, ..] => {
            (*id).to_owned()
        }
        // The long-lived `folderview` spelling that shared folders still go out as.
        ["folderview"] => query_value(path, "id")?.to_owned(),
        _ => return None,
    };
    valid_id(&id).then_some(Target { id })
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    fn folder(id: &str) -> Option<Target> {
        Some(Target { id: id.to_owned() })
    }

    #[test]
    fn every_spelling_of_a_folder_address_is_claimed() {
        let id = "1A2b3C4d5E6f7G8h9I0j";
        assert_eq!(
            claim(&format!("https://drive.google.com/drive/folders/{id}")),
            folder(id)
        );
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/drive/u/0/folders/{id}?usp=sharing"
            )),
            folder(id)
        );
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/drive/mobile/folders/{id}"
            )),
            folder(id)
        );
        assert_eq!(
            claim(&format!(
                "https://drive.google.com/drive/u/1/shared-drives/{id}"
            )),
            folder(id)
        );
        assert_eq!(
            claim(&format!("https://drive.google.com/folderview?id={id}")),
            folder(id)
        );
    }

    /// A file address belongs to the sibling resolver. Claiming it here would mean a listing
    /// request for every Drive download in the queue.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        let id = "1A2b3C4d5E6f7G8h9I0j";
        assert_eq!(
            claim(&format!("https://drive.google.com/file/d/{id}/view")),
            None
        );
        assert_eq!(
            claim(&format!("https://drive.google.com/open?id={id}")),
            None
        );
        assert_eq!(
            claim(&format!("https://docs.google.com/document/d/{id}/edit")),
            None
        );
        assert_eq!(
            claim(&format!(
                "https://www.googleapis.com/drive/v3/files/{id}?alt=media"
            )),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(
            claim("https://drive.google.com.evil.test/drive/folders/abc"),
            None
        );
        assert_eq!(claim("https://x@drive.google.com/drive/folders/abc"), None);
        assert_eq!(claim("ftp://drive.google.com/drive/folders/abc"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://drive.google.com/drive/my-drive"), None);
        assert_eq!(claim("https://drive.google.com/"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(
            claim("https://drive.google.com/drive/folders/../../api"),
            None
        );
        assert_eq!(claim("https://drive.google.com/drive/folders/a%2Fb"), None);
        assert_eq!(claim("https://drive.google.com/drive/folders/"), None);
        assert_eq!(claim("https://drive.google.com/folderview?id="), None);
    }
}
