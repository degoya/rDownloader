//! Deciding whether an address is a Box *folder*, and which one.
//!
//! Answered from the address alone, because `claims-url` is asked of every link a person pastes
//! and must reach nothing. The line against the sibling resolver is what Box spells into the
//! address — `/folder/` against `/file/` — with one address on neither side of it: a bare
//! `/s/<name>` shared link, which Box spells the same way whether it points at a file or at a
//! folder. That one is claimed here, because only the API can decide and this plugin has to ask
//! it either way; a crawl that finds a single file answers with that one file.

use box_common::address::{self, Address};

/// A Box folder this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A folder in the account's own Box, by its item id.
    Own { host: String, id: String },
    /// A shared link that has not said what it points at. The id is unknown until Box says so.
    Shared {
        link: String,
        password: Option<String>,
    },
    /// A folder reached through a shared link, by its item id inside that link.
    SharedFolder {
        link: String,
        id: String,
        password: Option<String>,
    },
}

impl Target {
    /// The shared link this target is reached through, if it is reached through one.
    #[must_use]
    pub fn link(&self) -> Option<&str> {
        match self {
            Self::Own { .. } => None,
            Self::Shared { link, .. } | Self::SharedFolder { link, .. } => Some(link),
        }
    }

    /// The `boxapi` header every request of this crawl needs, if it needs one.
    #[must_use]
    pub fn box_api(&self) -> Option<String> {
        match self {
            Self::Own { .. } => None,
            Self::Shared { link, password } | Self::SharedFolder { link, password, .. } => {
                address::box_api(link, password.as_deref())
            }
        }
    }

    /// The folder the walk starts at, or `None` for a shared link that has not said yet.
    #[must_use]
    pub fn start_id(&self) -> Option<&str> {
        match self {
            Self::Own { id, .. } | Self::SharedFolder { id, .. } => Some(id),
            Self::Shared { .. } => None,
        }
    }

    /// The canonical address of one file this crawl found: through the shared link it was found
    /// behind, or on the host the pasted address was on.
    ///
    /// A protected link's password goes with it, because an address is the only thing that
    /// passes between this package and the resolver — leaving it out would list the folder
    /// perfectly and then fail every file in it. It is a parameter `rd_core::REDACTED_QUERY`
    /// names, so it is struck out of every log line, and it never reaches the address the bytes
    /// come from.
    #[must_use]
    pub fn found_address(&self, id: &str) -> String {
        match self {
            Self::Own { host, .. } => address::file_address(host, id),
            Self::Shared { link, password } | Self::SharedFolder { link, password, .. } => {
                address::shared_file_address(link, id, password.as_deref())
            }
        }
    }
}

/// Reads an address, returning the folder this plugin would list — or `None`, which is what
/// `claims-url` answers for everything that is not a Box folder address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::Folder { host, id } => Some(Target::Own { host, id }),
        Address::Shared { link, password } => Some(Target::Shared { link, password }),
        Address::SharedFolder { link, id, password } => {
            Some(Target::SharedFolder { link, id, password })
        }
        // A file is the sibling resolver's, whichever way it is spelled.
        Address::File { .. } | Address::SharedFile { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    #[test]
    fn every_spelling_of_a_folder_address_is_claimed() {
        assert_eq!(
            claim("https://app.box.com/folder/987654321"),
            Some(Target::Own {
                host: "app.box.com".to_owned(),
                id: "987654321".to_owned()
            })
        );
        assert_eq!(
            claim("https://app.box.com/s/abc123?password=hunter2"),
            Some(Target::Shared {
                link: "https://app.box.com/s/abc123".to_owned(),
                password: Some("hunter2".to_owned())
            })
        );
        assert_eq!(
            claim("https://contoso.app.box.com/s/abc123/folder/7"),
            Some(Target::SharedFolder {
                link: "https://contoso.app.box.com/s/abc123".to_owned(),
                id: "7".to_owned(),
                password: None
            })
        );
    }

    /// A file belongs to the sibling resolver. This is the over-claim that would cost every
    /// download in the queue a listing request.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        assert_eq!(claim("https://app.box.com/file/123456789"), None);
        assert_eq!(claim("https://app.box.com/s/abc123/file/42"), None);
        assert_eq!(
            claim(&box_common::address::file_address("app.box.com", "7")),
            None
        );
        assert_eq!(
            claim(&box_common::address::shared_file_address(
                "https://app.box.com/s/abc123",
                "7",
                Some("hunter2")
            )),
            None
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        for foreign in [
            "https://app.box.com.evil.test/folder/1",
            "https://x@app.box.com/folder/1",
            "ftp://app.box.com/folder/1",
            "https://ddownload.com/f/abc",
            "https://app.box.com/",
            "https://app.box.com/folder/abc",
            "https://app.box.com/s/a%2Fb",
        ] {
            assert_eq!(claim(foreign), None, "{foreign}");
        }
    }

    /// What one crawl hands back is an address, and the shared link a file was found through
    /// travels with it — because a shared link grants access through the link and not by the
    /// item's own id.
    #[test]
    fn a_found_file_is_addressed_through_the_way_it_was_found() {
        let own = claim("https://contoso.app.box.com/folder/1").expect("a folder");
        assert_eq!(
            own.found_address("42"),
            "https://contoso.app.box.com/file/42"
        );
        assert_eq!(own.box_api(), None);
        assert_eq!(own.start_id(), Some("1"));

        let shared = claim("https://app.box.com/s/abc123?password=hunter2").expect("a link");
        assert_eq!(
            shared.found_address("42"),
            "https://app.box.com/s/abc123/file/42?shared_link_password=hunter2"
        );
        assert_eq!(
            shared.box_api().as_deref(),
            Some("shared_link=https://app.box.com/s/abc123&shared_link_password=hunter2")
        );
        assert_eq!(shared.start_id(), None);
        // The password travels in the hand-over address because nothing else passes between
        // the two packages, and it travels as a parameter the core strikes out of every log
        // line. What it never reaches is the address the bytes come from, which the resolver
        // builds out of an item id and a version alone.
        assert!(rd_core_redacts("shared_link_password"));
    }

    /// The parameter the hand-over uses has to be one the core treats as a credential. Spelled
    /// out here rather than imported: a plugin links nothing of the host's, so this repeats the
    /// list's answer and `crates/rd-plugin-ext/tests/box_contract.rs` asserts it against the
    /// real `rd_core::is_secret_parameter`.
    fn rd_core_redacts(name: &str) -> bool {
        name == "shared_link_password"
    }
}
