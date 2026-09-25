//! What a Seedr folder listing says, and the state machine hidden inside it.
//!
//! `GET /rest/folder` and `GET /rest/folder/{id}` answer with one shape, and that one shape is
//! also the only place a transfer's progress can be read to the end. Seedr's documented
//! `GET /rest/transfer/{id}` reports a *running* transfer; when the torrent finishes, Seedr
//! moves it out of the transfer list and into a folder, so the transfer endpoint can only stop
//! answering — which is indistinguishable from a transfer somebody deleted. The root listing
//! answers both questions at once, in one request, which is why the remote job polls it rather
//! than the endpoint named after polling.
//!
//! The field names are Seedr's own, as its published document and its own PHP example use them
//! (`$root_folder->folders[0]->id`, `$sub->files[0]->id`). Everything is optional and nothing
//! is required, because a listing that grew a field must not cost somebody a download.

use serde::Deserialize;

/// One folder listing.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Listing {
    /// Transfers that are still running. A finished one is not here any more.
    #[serde(default)]
    pub torrents: Vec<Torrent>,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub files: Vec<File>,
    /// The folder's own name, when Seedr states one. The root folder's is not meaningful.
    #[serde(default)]
    pub name: Option<String>,
}

impl Listing {
    /// Reads a listing out of a response body.
    ///
    /// **Only a JSON object is read**, and that is checked rather than assumed: serde builds a
    /// struct from a *sequence* as readily as from a map, taking the elements in field order,
    /// so a bare array would quietly become a listing whose `torrents` were its first element.
    /// The same trap cost `plugins/offcloud-cloud/` a finished job that arrived as an empty
    /// package, and it is not a fact about Offcloud.
    #[must_use]
    pub fn of(body: &[u8]) -> Option<Self> {
        let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
        if !value.is_object() {
            return None;
        }
        serde_json::from_value(value).ok()
    }
}

/// A transfer Seedr is still working on.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Torrent {
    #[serde(default, alias = "user_torrent_id")]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    /// Per cent, as Seedr counts it. Its own document warns that 100 means fully downloaded
    /// and **101 means moved to a folder**, which is why nothing here treats 100 as finished:
    /// the transfer leaving the list is what finished means.
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default, alias = "torrent_hash", alias = "info_hash")]
    pub hash: Option<String>,
    /// The folder this transfer has already created, when it has.
    #[serde(default, alias = "folder_id")]
    pub folder_created: Option<u64>,
    /// Seedr's own word for a stuck transfer, when it says one. Read only so a refusal can be
    /// detected; the sentence itself never travels.
    #[serde(default)]
    pub warnings: Option<serde_json::Value>,
}

/// A folder inside another folder.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Folder {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// One file, which is one download.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct File {
    #[serde(default, alias = "folder_file_id")]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// The progress of a transfer, in the thousandths the remote-job contract carries.
///
/// `None` rather than zero when Seedr stated nothing: a bar reading 0 % because nothing was
/// measured is a worse answer than no bar at all, and the contract has a way to say so. The
/// documented 101 is clamped rather than refused — it means the folder exists, which is past
/// the end rather than an error.
#[must_use]
pub fn permille(per_cent: Option<f64>) -> Option<u16> {
    let per_cent = per_cent?;
    if !per_cent.is_finite() {
        return None;
    }
    let scaled = (per_cent * 10.0).round().clamp(0.0, 1_000.0);
    // The clamp bounds the value into u16 range before the cast, so nothing is lost.
    Some(scaled as u16)
}

#[cfg(test)]
mod tests {
    use super::{Listing, permille};

    #[test]
    fn a_listing_carries_its_transfers_its_folders_and_its_files() {
        let listing = Listing::of(
            br#"{"torrents":[{"id":11,"name":"Example.Release","progress":42.5,
                 "torrent_hash":"DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"}],
                 "folders":[{"id":5,"name":"Example.Release","size":31}],
                 "files":[{"folder_file_id":9,"name":"readme.txt","size":31}]}"#,
        )
        .expect("a listing");
        assert_eq!(listing.torrents[0].id, Some(11));
        assert_eq!(listing.torrents[0].progress, Some(42.5));
        assert_eq!(listing.folders[0].id, Some(5));
        assert_eq!(listing.files[0].id, Some(9));
        assert_eq!(listing.files[0].name.as_deref(), Some("readme.txt"));
    }

    /// Serde builds a struct from a sequence as readily as from a map, so an answer that is an
    /// array would otherwise become a listing whose first element was its transfer list.
    #[test]
    fn an_answer_that_is_not_an_object_is_not_a_listing() {
        assert!(Listing::of(br#"["a","b"]"#).is_none());
        assert!(Listing::of(b"<html>502</html>").is_none());
    }

    #[test]
    fn progress_is_thousandths_and_says_nothing_when_seedr_said_nothing() {
        assert_eq!(permille(Some(0.0)), Some(0));
        assert_eq!(permille(Some(42.5)), Some(425));
        // Seedr's own document: 101 % means the folder has been created.
        assert_eq!(permille(Some(101.0)), Some(1_000));
        assert_eq!(permille(None), None);
        assert_eq!(permille(Some(f64::NAN)), None);
    }
}
