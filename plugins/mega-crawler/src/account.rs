//! A folder of the signed-in account, turned into files with names and places (RD-120-30).
//!
//! The same walk as a shared folder's, with one difference that decides everything about it:
//! **every key here is wrapped under the account's master key**, and the master key never
//! enters this sandbox. So this module does not open keys itself. It hands each wrapped key to
//! `unwrap` -- in the component, one `aes-ecb-decrypt` step over the session's key half, run by
//! the host -- and gets back that node's key and nothing wider.
//!
//! Pure, like `walk`: a parsed listing and a closure in, files out. The network, the refusals
//! and the WIT vocabulary are the guest's.

use std::collections::BTreeMap;

use mega_common::{
    api::Node,
    crypto::{self, Attributes, FileKey},
};
use serde_json::Value;

use crate::walk::{Found, MAX_DEPTH, MAX_FILES, clean};

/// Why an account listing produced nothing usable, or what the host said when asked to unwrap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal<E> {
    /// The named node is not in the account.
    NotInAccount,
    /// Nothing under it is a file whose key is the account's own.
    Empty,
    /// More files than [`MAX_FILES`], counted before a single key is unwrapped.
    TooMany,
    /// The host refused an unwrap; carried through unchanged.
    Host(E),
}

/// Everything under the node `handle` of the signed-in account.
///
/// A node the account holds through somebody else's share files its key under that share
/// and not under the account's own handle; it is skipped rather than guessed at, the way the
/// shared-folder walk skips what its key does not open.
pub fn expand<E>(
    listing: &Value,
    handle: &str,
    mut unwrap: impl FnMut(&[u8]) -> Result<Vec<u8>, E>,
) -> Result<Vec<Found>, Refusal<E>> {
    let nodes = Node::list(listing);
    let root = nodes
        .iter()
        .find(|node| node.handle == handle)
        .ok_or(Refusal::NotInAccount)?;
    let parents: BTreeMap<&str, &str> = nodes
        .iter()
        .map(|node| (node.handle.as_str(), node.parent.as_str()))
        .collect();
    let under: Vec<&Node> = nodes
        .iter()
        .filter(|node| node.handle == handle || descends_from(&node.handle, handle, &parents))
        .collect();
    // Counted before anything is unwrapped: a refusal should cost one listing, not a
    // thousand host calls.
    if under.iter().filter(|node| node.kind == 0).count() > MAX_FILES {
        return Err(Refusal::TooMany);
    }
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut files = Vec::new();
    for node in under {
        let Some(wrapped) = node.own_key().and_then(crypto::b64_decode) else {
            continue;
        };
        let expected = if node.kind == 0 { 32 } else { 16 };
        if wrapped.len() != expected {
            continue;
        }
        let raw = unwrap(&wrapped).map_err(Refusal::Host)?;
        let key = match node.kind {
            0 => match FileKey::from_raw(&raw) {
                Some(key) => key.key,
                None => continue,
            },
            _ => match <[u8; 16]>::try_from(raw) {
                Ok(key) => key,
                Err(_) => continue,
            },
        };
        let name = crypto::b64_decode(&node.attributes)
            .as_deref()
            .and_then(|blob| crypto::decrypt_attributes(&key, blob))
            .and_then(|plain| clean(Attributes::parse(&plain).name.as_deref()));
        let Some(name) = name else { continue };
        names.insert(node.handle.clone(), name.clone());
        if node.kind == 0 {
            files.push((node.handle.clone(), name, node.size));
        }
    }
    let found: Vec<Found> = files
        .into_iter()
        .map(|(node, name, size)| Found {
            path: path_of(&node, &root.handle, root.kind == 0, &parents, &names),
            node,
            name,
            size,
        })
        .collect();
    if found.is_empty() {
        return Err(Refusal::Empty);
    }
    Ok(found)
}

/// Whether `node` hangs, at most [`MAX_DEPTH`] levels down, off `ancestor`.
fn descends_from(node: &str, ancestor: &str, parents: &BTreeMap<&str, &str>) -> bool {
    let mut current = node;
    for _ in 0..MAX_DEPTH {
        let Some(parent) = parents.get(current) else {
            return false;
        };
        if *parent == ancestor {
            return true;
        }
        current = parent;
    }
    false
}

/// The named folder's own name, then the path down to one file. Empty for a single file.
fn path_of(
    node: &str,
    root: &str,
    root_is_file: bool,
    parents: &BTreeMap<&str, &str>,
    names: &BTreeMap<String, String>,
) -> String {
    if root_is_file {
        return String::new();
    }
    let mut segments = Vec::new();
    let mut current = parents.get(node).copied().unwrap_or_default();
    for _ in 0..MAX_DEPTH {
        let Some(name) = names.get(current) else {
            break;
        };
        segments.push(name.as_str());
        if current == root {
            break;
        }
        current = parents.get(current).copied().unwrap_or_default();
    }
    segments.reverse();
    segments.join("/")
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod account_tests;
