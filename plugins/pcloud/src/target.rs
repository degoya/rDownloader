//! Deciding whether an address is a pCloud *file*, and which one.
//!
//! Answered from the address alone, because `match-url` is asked of every link a person pastes
//! and must reach nothing. What it must not do is over-claim: a folder address and a bare
//! public link belong to the sibling crawler, and a resolver that swallowed them would turn a
//! folder of two hundred files into one refusal.
//!
//! The line between the two is `fileid`, and pCloud leaves no other: a public link code is
//! opaque, so nothing in an address says whether a link points at a file or at a folder. What
//! an address *can* say is whether a particular file is named — which is the very parameter
//! `getpublinkdownload` requires once a link points at a folder. So a bare public link goes to
//! the crawler even when it turns out to hold a single file, and comes back from there as that
//! one file, spelled with its `fileid`.

use pcloud_common::address::{self, Address, Region};

/// A pCloud file this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A file in the account's own drive, by its `fileid`.
    Own { file_id: u64, region: Region },
    /// A file behind a public link — the link's code and the file inside it.
    Public {
        code: String,
        file_id: u64,
        region: Region,
    },
}

/// Reads an address, returning the file this plugin would fetch — or `None`, which is what
/// `match-url` answers for everything that is not a pCloud *file* address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::Own {
            file_id: Some(file_id),
            region,
            ..
        } => Some(Target::Own { file_id, region }),
        Address::Public {
            code,
            file_id: Some(file_id),
            region,
            ..
        } => Some(Target::Public {
            code,
            file_id,
            region,
        }),
        // A folder or a bare public link — the crawler's, whichever way it is spelled.
        Address::Own { file_id: None, .. } | Address::Public { file_id: None, .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Region, Target, claim};

    #[test]
    fn every_spelling_of_a_file_address_is_claimed() {
        assert_eq!(
            claim("https://my.pcloud.com/#/filemanager?folder=42&fileid=123"),
            Some(Target::Own {
                file_id: 123,
                region: Region::Us,
            })
        );
        assert_eq!(
            claim("https://e.pcloud.com/#/filemanager?folder=0&fileid=1"),
            Some(Target::Own {
                file_id: 1,
                region: Region::Eu,
            })
        );
        assert_eq!(
            claim("https://e.pcloud.link/publink/show?code=XZabc&fileid=7"),
            Some(Target::Public {
                code: "XZabc".to_owned(),
                file_id: 7,
                region: Region::Eu,
            })
        );
        // The canonical addresses the sibling crawler hands back.
        assert_eq!(
            claim(&pcloud_common::address::file_address(Region::Eu, 42, 123)),
            Some(Target::Own {
                file_id: 123,
                region: Region::Eu,
            })
        );
        assert_eq!(
            claim(&pcloud_common::address::public_file_address(
                Region::Us,
                "XZabc",
                7
            )),
            Some(Target::Public {
                code: "XZabc".to_owned(),
                file_id: 7,
                region: Region::Us,
            })
        );
    }

    /// A folder and a bare public link belong to the sibling crawler. This is the over-claim
    /// that would turn a folder of two hundred files into one refusal.
    #[test]
    fn a_folder_and_a_bare_public_link_are_left_to_the_crawler() {
        assert_eq!(claim("https://my.pcloud.com/#/filemanager?folder=42"), None);
        assert_eq!(claim("https://e.pcloud.link/publink/show?code=XZabc"), None);
        assert_eq!(
            claim("https://my.pcloud.com/#page=publink&code=XZabc"),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(
            claim("https://pcloud.com.evil.test/publink/show?code=X&fileid=1"),
            None
        );
        assert_eq!(
            claim("https://x@my.pcloud.com/#/filemanager?folder=1&fileid=2"),
            None
        );
        assert_eq!(
            claim("ftp://my.pcloud.com/#/filemanager?folder=1&fileid=2"),
            None
        );
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://my.pcloud.com/"), None);
        assert_eq!(claim("https://api.pcloud.com/getfilelink?fileid=1"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(
            claim("https://my.pcloud.com/#/filemanager?folder=1&fileid=../etc"),
            None
        );
        assert_eq!(
            claim("https://e.pcloud.link/publink/show?code=a%2Fb&fileid=1"),
            None
        );
        assert_eq!(
            claim("https://my.pcloud.com/#/filemanager?folder=1&fileid="),
            None
        );
    }
}
