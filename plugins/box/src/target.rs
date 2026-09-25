//! Deciding whether an address is a Box *file*, and which one.
//!
//! Answered from the address alone, because `match-url` is asked of every link a person pastes
//! and must reach nothing. What it must not do is over-claim: a folder address belongs to the
//! sibling crawler, and a resolver that swallowed it would turn a folder of two hundred files
//! into one refusal. The line between the two is what Box spells into the address — `/file/`
//! against `/folder/` — with one address on neither side of it: a bare `/s/<name>` shared link,
//! which Box spells the same way whether it points at a file or at a folder. That one is the
//! crawler's, because it has to ask the API either way and a crawl that finds a single file
//! answers with that one file.

use box_common::address::{self, Address};

/// A Box file this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A file in the account's own Box, by its item id.
    Own { id: String },
    /// A file reached through a shared link, by its item id inside that link. The link grants
    /// the access, so it travels with every request as the `boxapi` header.
    Shared {
        link: String,
        id: String,
        password: Option<String>,
    },
}

impl Target {
    /// The item id, whichever way the file is reached.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Own { id } | Self::Shared { id, .. } => id,
        }
    }

    /// Whether this file is reached through a shared link, which is what decides how a refusal
    /// is reported.
    #[must_use]
    pub const fn is_shared(&self) -> bool {
        matches!(self, Self::Shared { .. })
    }

    /// The `boxapi` header this target needs, if it needs one.
    #[must_use]
    pub fn box_api(&self) -> Option<String> {
        match self {
            Self::Own { .. } => None,
            Self::Shared { link, password, .. } => address::box_api(link, password.as_deref()),
        }
    }
}

/// Reads an address, returning the file this plugin would fetch — or `None`, which is what
/// `match-url` answers for everything that is not a Box *file* address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::File { id, .. } => Some(Target::Own { id }),
        Address::SharedFile { link, id, password } => Some(Target::Shared { link, id, password }),
        // A folder, or a shared link that has not said what it is: the crawler's, both of them.
        Address::Folder { .. } | Address::Shared { .. } | Address::SharedFolder { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    #[test]
    fn every_spelling_of_a_file_address_is_claimed() {
        assert_eq!(
            claim("https://app.box.com/file/123456789"),
            Some(Target::Own {
                id: "123456789".to_owned()
            })
        );
        assert_eq!(
            claim("https://contoso.app.box.com/file/1"),
            Some(Target::Own { id: "1".to_owned() })
        );
        // A file inside a shared link: the link plus the item inside it, and the password the
        // address carried.
        assert_eq!(
            claim("https://app.box.com/s/abc123/file/42?shared_link_password=hunter2"),
            Some(Target::Shared {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "42".to_owned(),
                password: Some("hunter2".to_owned()),
            })
        );
        // The canonical addresses the sibling crawler hands back.
        assert_eq!(
            claim(&box_common::address::file_address("app.box.com", "7")),
            Some(Target::Own { id: "7".to_owned() })
        );
        assert_eq!(
            claim(&box_common::address::shared_file_address(
                "https://app.box.com/s/abc123",
                "7",
                None
            )),
            Some(Target::Shared {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "7".to_owned(),
                password: None,
            })
        );
    }

    /// A folder belongs to the sibling crawler. This is the over-claim that would turn a folder
    /// of two hundred files into one refusal.
    /// A protected link's file address, as the sibling crawler hands it over: the password
    /// comes back out of it and goes straight into the `boxapi` header.
    #[test]
    fn a_crawled_file_behind_a_protected_link_keeps_its_password() {
        let claimed = claim(&box_common::address::shared_file_address(
            "https://app.box.com/s/abc123",
            "7",
            Some("hunter 2&x"),
        ))
        .expect("a shared file");
        assert_eq!(
            claimed.box_api().as_deref(),
            Some("shared_link=https://app.box.com/s/abc123&shared_link_password=hunter%202%26x")
        );
    }

    #[test]
    fn a_folder_address_is_left_to_the_crawler() {
        assert_eq!(claim("https://app.box.com/folder/987"), None);
        assert_eq!(claim("https://app.box.com/s/abc123/folder/7"), None);
        // And the one address Box does not say the kind of: the crawler's, because only the
        // API can decide and it has to ask anyway.
        assert_eq!(claim("https://app.box.com/s/abc123"), None);
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        for foreign in [
            "https://app.box.com.evil.test/file/1",
            "https://x@app.box.com/file/1",
            "ftp://app.box.com/file/1",
            "https://ddownload.com/f/abc",
            "https://app.box.com/",
            "https://api.box.com/2.0/files/1/content",
        ] {
            assert_eq!(claim(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(claim("https://app.box.com/file/../etc/passwd"), None);
        assert_eq!(claim("https://app.box.com/file/abc"), None);
        assert_eq!(claim("https://app.box.com/s/a%2Fb/file/1"), None);
        assert_eq!(claim("https://app.box.com/file/"), None);
    }

    /// The password reaches the `boxapi` header and nothing else.
    #[test]
    fn the_password_leaves_this_plugin_only_in_the_box_api_header() {
        let claimed =
            claim("https://app.box.com/s/abc123/file/42?password=hunter2").expect("a shared file");
        assert!(claimed.is_shared());
        assert_eq!(
            claimed.box_api().as_deref(),
            Some("shared_link=https://app.box.com/s/abc123&shared_link_password=hunter2")
        );
        assert_eq!(claimed.id(), "42");
        let own = claim("https://app.box.com/file/42").expect("an own file");
        assert_eq!(own.box_api(), None);
    }
}
