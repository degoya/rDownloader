//! Which MEGA address is which, without fetching anything.
//!
//! Both plugins are asked `claims-url` before they are allowed to reach the network, so this
//! module decides from the text alone. Four forms exist in the wild and all four are read
//! here: the current `/file/` and `/folder/` addresses, a file inside a folder, and the two
//! legacy `#!` and `#F!` forms MEGA has been redirecting for years but still hands out in old
//! posts.
//!
//! Two more name a node of the **signed-in account** (RD-120-30), and carry no key at all --
//! the key is wrapped under the account's master key, which only the host holds:
//!
//! * `https://mega.nz/fm/<handle>` -- a folder (or file) of the account, the crawler's;
//! * `https://mega.nz/fm/file/<handle>` -- one file of the account, the stream plugin's.
//!
//! The first is the path MEGA's own web client shows for a folder of the cloud drive. The
//! second is this project's own spelling for what the crawler hands on, chosen so the two
//! plugins can never both claim one address; `file` is not a handle (too short), so it cannot
//! be mistaken for one. **Neither was measured against a live account.**

/// Hosts MEGA serves its web addresses on.
pub const HOSTS: [&str; 2] = ["mega.nz", "mega.co.nz"];

/// The API endpoint every request goes to.
pub const API: &str = "https://g.api.mega.co.nz/cs";

/// The host pattern storage nodes live under; the manifest grants exactly this.
pub const STORAGE_SUFFIX: &str = ".userstorage.mega.co.nz";

/// What an address points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A file shared on its own. The fragment carries its 32-byte key.
    File { handle: String, key: String },
    /// A folder. The fragment carries the 16-byte share key.
    Folder { handle: String, key: String },
    /// One file inside a shared folder. Its key is not in the address: it sits in the
    /// folder's node list, encrypted under the share key.
    FolderChild {
        folder: String,
        key: String,
        node: String,
    },
    /// A node of the signed-in account, file or folder. Its key is under the master key.
    AccountNode { handle: String },
    /// One file of the signed-in account, as the crawler hands it on.
    AccountFile { handle: String },
}

impl Target {
    /// Reads an address, or `None` when it is not MEGA's.
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let rest = strip_host(url)?;
        let (path, fragment) = match rest.split_once('#') {
            Some((path, fragment)) => (path, fragment),
            None => (rest, ""),
        };
        let path = path.trim_start_matches('/');
        if let Some(account) = parse_account(path, fragment) {
            return account;
        }
        if let Some(legacy) = parse_legacy(fragment) {
            return Some(legacy);
        }
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let kind = segments.next()?;
        let handle = segments.next()?;
        if !is_handle(handle) {
            return None;
        }
        // A folder address may name a file inside it, in the path after the fragment:
        // `…/folder/<handle>#<key>/file/<node>`. The fragment is therefore split too.
        let (key, inner) = match fragment.split_once('/') {
            Some((key, inner)) => (key, Some(inner)),
            None => (fragment, None),
        };
        if !is_key(key) {
            return None;
        }
        match kind {
            "file" => Some(Self::File {
                handle: handle.to_owned(),
                key: key.to_owned(),
            }),
            "folder" => {
                let node = inner
                    .and_then(|inner| inner.strip_prefix("file/"))
                    .map(|node| node.split('/').next().unwrap_or(node));
                match node {
                    Some(node) if is_handle(node) => Some(Self::FolderChild {
                        folder: handle.to_owned(),
                        key: key.to_owned(),
                        node: node.to_owned(),
                    }),
                    Some(_) => None,
                    None => Some(Self::Folder {
                        handle: handle.to_owned(),
                        key: key.to_owned(),
                    }),
                }
            }
            _ => None,
        }
    }

    /// The address a crawler emits for one file it found inside a folder.
    #[must_use]
    pub fn child_url(folder: &str, key: &str, node: &str) -> String {
        format!("https://mega.nz/folder/{folder}#{key}/file/{node}")
    }

    /// The address a crawler emits for one file of the signed-in account. No key: that one
    /// stays wrapped until the host unwraps it for the plugin resolving this address.
    #[must_use]
    pub fn account_file_url(handle: &str) -> String {
        format!("https://mega.nz/fm/file/{handle}")
    }
}

/// The two account forms, `fm/<handle>` and `fm/file/<handle>`.
///
/// `None` when the path is not under `fm/` at all, so the other forms get their turn;
/// `Some(None)` when it is and is malformed -- a fragment, a stray segment, a handle outside
/// MEGA's alphabet -- which is nobody's address.
fn parse_account(path: &str, fragment: &str) -> Option<Option<Target>> {
    let rest = path.strip_prefix("fm/")?;
    let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    if !fragment.is_empty() {
        return Some(None);
    }
    Some(match segments.as_slice() {
        ["file", handle] if is_handle(handle) => Some(Target::AccountFile {
            handle: (*handle).to_owned(),
        }),
        [handle] if is_handle(handle) => Some(Target::AccountNode {
            handle: (*handle).to_owned(),
        }),
        _ => None,
    })
}

/// Whatever follows a MEGA host, or `None` for any other address.
fn strip_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    HOSTS.iter().find_map(|host| {
        rest.strip_prefix(host)
            .filter(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('#'))
    })
}

/// The two forms MEGA used before 2020: `#!<handle>!<key>` and `#F!<handle>!<key>`.
fn parse_legacy(fragment: &str) -> Option<Target> {
    let (folder, rest) = match fragment.strip_prefix("F!") {
        Some(rest) => (true, rest),
        None => (false, fragment.strip_prefix('!')?),
    };
    let (handle, key) = rest.split_once('!')?;
    let key = key.split(['!', '/']).next().unwrap_or(key);
    if !is_handle(handle) || !is_key(key) {
        return None;
    }
    Some(if folder {
        Target::Folder {
            handle: handle.to_owned(),
            key: key.to_owned(),
        }
    } else {
        Target::File {
            handle: handle.to_owned(),
            key: key.to_owned(),
        }
    })
}

/// A node handle: six to eleven characters of MEGA's base64 alphabet.
#[must_use]
pub fn is_handle(value: &str) -> bool {
    (6..=11).contains(&value.len()) && value.chars().all(is_b64_character)
}

/// A key as it appears in a fragment: 22 characters for a folder, 43 for a file.
#[must_use]
pub fn is_key(value: &str) -> bool {
    matches!(value.len(), 22 | 43) && value.chars().all(is_b64_character)
}

fn is_b64_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod address_tests;
