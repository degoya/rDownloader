//! Deciding whether an address is a OneDrive or SharePoint *file*, and which one.
//!
//! Answered from the address alone, because `match-url` is asked of every link a person pastes
//! and must reach nothing. What it must not do is over-claim: a folder link belongs to the
//! sibling crawler, and so does a long `onedrive.live.com` address that does not say what it
//! points at — a resolver that swallowed either would turn a folder of two hundred files into
//! one refusal.

use onedrive_common::address::{
    Host, LinkKind, microsoft_host, segments, share_id, share_link_kind, split, valid_id,
    valid_share_id,
};

/// An address this plugin claims, reduced to the Graph route that reaches it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A sharing link, as pasted: reached through `/shares/{id}/driveItem`.
    Share { share_id: String },
    /// One item inside a shared folder, as the sibling crawler emits it: reached through
    /// `/shares/{id}/items/{item}`.
    SharedItem { share_id: String, item_id: String },
    /// An item by its own drive and id, as somebody who copied a Graph address would paste it:
    /// reached through `/drives/{drive}/items/{item}`.
    Item { drive_id: String, item_id: String },
}

impl Target {
    /// The Graph route this target is read and downloaded through, without `/content`.
    #[must_use]
    pub fn route(&self) -> String {
        let api = onedrive_common::address::GRAPH;
        match self {
            Self::Share { share_id } => format!("{api}/shares/{share_id}/driveItem"),
            Self::SharedItem { share_id, item_id } => {
                format!("{api}/shares/{share_id}/items/{item_id}")
            }
            Self::Item { drive_id, item_id } => {
                format!("{api}/drives/{drive_id}/items/{item_id}")
            }
        }
    }
}

/// Reads an address, returning the item this plugin would fetch — or `None`, which is what
/// `match-url` answers for everything that is not a OneDrive or SharePoint *file* address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    let (host, path) = split(url)?;
    let host = microsoft_host(host)?;
    if host == Host::Graph {
        return claim_graph(path);
    }
    // A sharing link. Only the kinds Microsoft spelled as a file: a folder is the crawler's,
    // and so is the long address that says nothing — the crawler asks, and answers with one
    // file when that is what it finds.
    match share_link_kind(host, path)? {
        LinkKind::File => Some(Target::Share {
            share_id: share_id(url)?,
        }),
        LinkKind::Folder | LinkKind::Unknown => None,
    }
}

/// The canonical Graph spellings: what the sibling crawler emits, and what somebody copies out
/// of the API explorer. Anything else on that host is not an item.
fn claim_graph(path: &str) -> Option<Target> {
    let parts = segments(path);
    match parts.as_slice() {
        ["v1.0", "shares", share, "driveItem"]
        | ["v1.0", "shares", share, "driveItem", "content"]
            if valid_share_id(share) =>
        {
            Some(Target::Share {
                share_id: (*share).to_owned(),
            })
        }
        ["v1.0", "shares", share, "items", item]
        | ["v1.0", "shares", share, "items", item, "content"]
            if valid_share_id(share) && valid_id(item) =>
        {
            Some(Target::SharedItem {
                share_id: (*share).to_owned(),
                item_id: (*item).to_owned(),
            })
        }
        ["v1.0", "drives", drive, "items", item]
        | ["v1.0", "drives", drive, "items", item, "content"]
            if valid_id(drive) && valid_id(item) =>
        {
            Some(Target::Item {
                drive_id: (*drive).to_owned(),
                item_id: (*item).to_owned(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};
    use onedrive_common::address::{item_address, root_address, share_id};

    const SHARE: &str = "u!aHR0cHM6Ly8xZHJ2Lm1zL3UvcyFBa1h5";

    #[test]
    fn every_spelling_of_a_file_sharing_link_is_claimed_as_its_share_id() {
        for link in [
            "https://1drv.ms/u/s!AkXy_Zabc-DEF",
            "https://1drv.ms/w/c/ab12cd34ef56/EaBcDeFgHiJ?e=abc123",
            "https://contoso-my.sharepoint.com/:b:/g/personal/someone_contoso_com/EaBcDeF?e=x",
            "https://contoso.sharepoint.com/:x:/s/Team/EaBcDeF",
            "https://onedrive.live.com/view.aspx?resid=ABC123%21456&cid=ABC123",
        ] {
            assert_eq!(
                claim(link),
                Some(Target::Share {
                    share_id: share_id(link).expect("encoded"),
                }),
                "{link}"
            );
        }
    }

    /// The canonical addresses the sibling crawler hands back, and the Graph address somebody
    /// might paste by hand.
    #[test]
    fn the_canonical_graph_addresses_are_claimed() {
        assert_eq!(
            claim(&root_address(SHARE)),
            Some(Target::Share {
                share_id: SHARE.to_owned(),
            })
        );
        assert_eq!(
            claim(&item_address(SHARE, "01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K")),
            Some(Target::SharedItem {
                share_id: SHARE.to_owned(),
                item_id: "01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K".to_owned(),
            })
        );
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/drives/b!abcDEF-123/items/01BYE5RZ/content"),
            Some(Target::Item {
                drive_id: "b!abcDEF-123".to_owned(),
                item_id: "01BYE5RZ".to_owned(),
            })
        );
        // And the route each one is read through is the address itself.
        assert_eq!(
            claim(&item_address(SHARE, "01BYE5RZ"))
                .expect("claimed")
                .route(),
            item_address(SHARE, "01BYE5RZ")
        );
    }

    /// A folder belongs to the sibling crawler, and so does the long personal address that does
    /// not say what it points at. This is the over-claim that would turn a folder of two
    /// hundred files into one refusal.
    #[test]
    fn a_folder_link_and_an_undecidable_one_are_left_to_the_crawler() {
        assert_eq!(claim("https://1drv.ms/f/s!AkXy_Zabc-DEF"), None);
        assert_eq!(
            claim("https://contoso.sharepoint.com/:f:/g/personal/someone_contoso_com/EaBc?e=1"),
            None
        );
        assert_eq!(
            claim("https://onedrive.live.com/?id=ABC123%21456&cid=ABC123"),
            None
        );
        assert_eq!(
            claim("https://onedrive.live.com/redir?resid=ABC123%21456&authkey=%21AB"),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(claim("https://1drv.ms.evil.test/u/s!abc"), None);
        assert_eq!(claim("https://x@1drv.ms/u/s!abc"), None);
        assert_eq!(claim("ftp://1drv.ms/u/s!abc"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://1drv.ms/"), None);
        assert_eq!(claim("https://graph.microsoft.com/v1.0/me"), None);
        assert_eq!(
            claim("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
            None
        );
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/drives/../items/x"),
            None
        );
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/shares/aHR0/driveItem"),
            None,
            "a share id without its prefix is not one"
        );
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/shares/u!a%2Fb/items/x"),
            None
        );
        assert_eq!(
            claim(&format!(
                "https://graph.microsoft.com/v1.0/drives/{}/items/x",
                "x".repeat(257)
            )),
            None
        );
    }
}
