//! Deciding whether an address is a pCloud *folder* or a bare public link.
//!
//! The mirror image of the sibling resolver's decision, and the reason both are narrow:
//! `claims-url` is asked of every link a person pastes, and a crawler that claimed a file
//! address would fetch a listing for every download in the queue. The line is `fileid`: an
//! address without one is here, an address with one is the resolver's.

use pcloud_common::address::{self, Address, Region};

/// A place this plugin lists, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A folder in the account's own drive, by its `folderid` — `0` is the root.
    Own { folder_id: u64, region: Region },
    /// A public link, whatever it turns out to point at.
    Public { code: String, region: Region },
}

/// Reads an address, returning the place this plugin would list — or `None`, which is what
/// `claims-url` answers for everything else.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::Own {
            folder_id,
            file_id: None,
            region,
            ..
        } => Some(Target::Own { folder_id, region }),
        Address::Public {
            code,
            file_id: None,
            region,
            ..
        } => Some(Target::Public { code, region }),
        // A file — the resolver's, whichever way it is spelled.
        Address::Own { .. } | Address::Public { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Region, Target, claim};

    #[test]
    fn every_spelling_of_a_folder_or_link_address_is_claimed() {
        assert_eq!(
            claim("https://my.pcloud.com/#/filemanager?folder=42"),
            Some(Target::Own {
                folder_id: 42,
                region: Region::Us,
            })
        );
        assert_eq!(
            claim("https://e.pcloud.com/#/filemanager?folder=0"),
            Some(Target::Own {
                folder_id: 0,
                region: Region::Eu,
            })
        );
        assert_eq!(
            claim("https://my.pcloud.com/#page=publink&code=XZabc"),
            Some(Target::Public {
                code: "XZabc".to_owned(),
                region: Region::Us,
            })
        );
        assert_eq!(
            claim("https://e.pcloud.link/publink/show?code=XZabc"),
            Some(Target::Public {
                code: "XZabc".to_owned(),
                region: Region::Eu,
            })
        );
        assert_eq!(
            claim(&pcloud_common::address::folder_address(Region::Eu, 9)),
            Some(Target::Own {
                folder_id: 9,
                region: Region::Eu,
            })
        );
    }

    /// A file address belongs to the sibling resolver. Claiming it here would mean a listing
    /// request for every pCloud download in the queue.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        assert_eq!(
            claim("https://my.pcloud.com/#/filemanager?folder=42&fileid=1"),
            None
        );
        assert_eq!(
            claim("https://e.pcloud.link/publink/show?code=XZabc&fileid=7"),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(
            claim("https://pcloud.com.evil.test/#/filemanager?folder=1"),
            None
        );
        assert_eq!(
            claim("https://x@my.pcloud.com/#/filemanager?folder=1"),
            None
        );
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://my.pcloud.com/"), None);
        assert_eq!(claim("https://api.pcloud.com/listfolder?folderid=1"), None);
        assert_eq!(claim("https://my.pcloud.com/#/filemanager?folder=x"), None);
    }
}
