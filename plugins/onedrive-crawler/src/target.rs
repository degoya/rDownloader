//! Deciding whether an address is a OneDrive or SharePoint *folder* link — or one that does
//! not say.
//!
//! The mirror image of the sibling resolver's decision, and the reason both are narrow:
//! `claims-url` is asked of every link a person pastes, and a crawler that claimed a file
//! address would fetch a listing for every download in the queue. The one place the line is
//! drawn differently from Google Drive: a long `onedrive.live.com` address carries no type
//! letter, so nobody can tell from it whether it names a folder or a file. The crawler takes
//! those, because it has to ask Graph anyway, and a crawl that finds one file hands back one
//! file — the resolver never sees the pasted address, only the canonical one behind it.

use onedrive_common::address::{Host, LinkKind, microsoft_host, share_id, share_link_kind, split};

/// A sharing link this plugin claims, reduced to what Graph needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    /// The share id Graph resolves the link through, as Microsoft encodes it.
    pub share_id: String,
    /// Whether the address said it was a folder, or said nothing. A crawl of an address that
    /// said nothing may legitimately find a single file.
    pub kind: LinkKind,
}

/// Reads an address, returning the link this plugin would list — or `None`, which is what
/// `claims-url` answers for everything the sibling resolver takes and for everything that is
/// nobody's.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    let (host, path) = split(url)?;
    let host = microsoft_host(host)?;
    if host == Host::Graph {
        // A Graph address is the resolver's spelling of a single item, and the address this
        // plugin itself hands back. Listing it would be a loop with extra steps.
        return None;
    }
    let kind = share_link_kind(host, path)?;
    if kind == LinkKind::File {
        return None;
    }
    Some(Target {
        share_id: share_id(url)?,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};
    use onedrive_common::address::{LinkKind, share_id};

    fn folder(link: &str) -> Option<Target> {
        Some(Target {
            share_id: share_id(link).expect("encoded"),
            kind: LinkKind::Folder,
        })
    }

    #[test]
    fn every_spelling_of_a_folder_link_is_claimed() {
        for link in [
            "https://1drv.ms/f/s!AkXy_Zabc-DEF",
            "https://1drv.ms/f/c/ab12cd34ef56/EaBcDeFgHiJ?e=abc123",
            "https://contoso-my.sharepoint.com/:f:/g/personal/someone_contoso_com/EaBcDeF?e=x",
            "https://contoso.sharepoint.com/:f:/s/Team/EaBcDeF",
            "https://contoso.sharepoint.com/:f:/r/sites/Team/Shared%20Documents/Reports",
        ] {
            assert_eq!(claim(link), folder(link), "{link}");
        }
    }

    /// The long personal address does not say what it points at, so it is claimed and asked.
    #[test]
    fn an_address_that_does_not_say_is_claimed_and_asked() {
        for link in [
            "https://onedrive.live.com/?id=ABC123%21456&cid=ABC123",
            "https://onedrive.live.com/redir?resid=ABC123%21456&authkey=%21AB",
            "https://onedrive.live.com/embed?cid=ABC123&resid=ABC123%21456",
        ] {
            assert_eq!(
                claim(link),
                Some(Target {
                    share_id: share_id(link).expect("encoded"),
                    kind: LinkKind::Unknown,
                }),
                "{link}"
            );
        }
    }

    /// A file address belongs to the sibling resolver. Claiming it here would mean a listing
    /// request for every OneDrive download in the queue.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        assert_eq!(claim("https://1drv.ms/u/s!AkXy_Zabc-DEF"), None);
        assert_eq!(claim("https://1drv.ms/w/c/ab12cd34ef56/EaBcDeF"), None);
        assert_eq!(
            claim("https://contoso.sharepoint.com/:b:/s/Team/EaBcDeF"),
            None
        );
        assert_eq!(
            claim("https://onedrive.live.com/view.aspx?resid=ABC123%21456"),
            None
        );
        // And the canonical addresses this plugin itself hands back.
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/shares/u!aHR0/items/01ABC"),
            None
        );
        assert_eq!(
            claim("https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem"),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(claim("https://1drv.ms.evil.test/f/s!abc"), None);
        assert_eq!(claim("https://x@1drv.ms/f/s!abc"), None);
        assert_eq!(claim("ftp://1drv.ms/f/s!abc"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://1drv.ms/"), None);
        assert_eq!(claim("https://onedrive.live.com/about/en-us/"), None);
        assert_eq!(
            claim(
                "https://contoso.sharepoint.com/sites/Team/Shared%20Documents/Forms/AllItems.aspx"
            ),
            None
        );
    }
}
