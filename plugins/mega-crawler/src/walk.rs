//! A folder's node list, turned into files with names and places.
//!
//! MEGA answers `a=f` with **every** node of a folder in one response: no cursor, no page
//! token -- measured on 2026-09-21 and again on 2026-09-22. So rule 4 of the shared
//! cloud-source interface ("at most ten pages per folder") has nothing to page through here,
//! and the bound is the size of that one answer instead. Two bounds hold it: the manifest's
//! `max_response_bytes`, which the host enforces before a byte of this module runs, and
//! [`MAX_FILES`], which refuses a listing rather than handing the LinkGrabber a package
//! nobody asked for.
//!
//! This module is pure. It takes a parsed listing and a share key and gives back files; the
//! network, the refusals and the WIT vocabulary are the guest's.

use std::collections::BTreeMap;

use mega_common::{
    api::{self, Node},
    crypto::{self, Attributes, FileKey},
};
use serde_json::Value;

/// The most files one crawl hands back.
///
/// Deliberately the *host's* own cap (`MAX_CRAWLED_LINKS`, RD-104-03) and not a number of
/// this plugin's choosing. The host trims a crawler's answer to that many links silently --
/// it has to, because a crawler's word about its own limits is not something a host may take
/// -- and a silently trimmed package is the same lie as an empty one: the person is handed
/// something that looks complete and is not. Refusing at the same number instead means a
/// folder either comes back whole or comes back as `mega_crawler.too_many_files` with the
/// limit in it. It was 5 000 until RD-120-11 measured what happened between the two.
pub const MAX_FILES: usize = 1_000;

/// The deepest a path is followed. A listing is a parent-pointer graph, not a tree, so a
/// cycle is possible in principle and this is what stops one.
pub const MAX_DEPTH: usize = 32;

/// One file behind a folder address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub node: String,
    pub name: String,
    pub size: u64,
    /// The folder's own name, then the path down to this file. What the host reads the
    /// outermost segment of as the package suggestion.
    pub path: String,
}

/// Why a listing produced nothing usable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The answer has no node that everything else hangs off.
    NoRoot,
    /// Nothing in it is a file this key opens.
    Empty,
    /// More files than [`MAX_FILES`].
    TooMany,
}

/// Everything behind a folder listing, in the order the answer named it.
pub fn expand(listing: &Value, share: &[u8; 16]) -> Result<Vec<Found>, Refusal> {
    let nodes = Node::list(listing);
    let root = api::share_root(&nodes).ok_or(Refusal::NoRoot)?.clone();
    // Every node's key is filed under the share root's handle; an entry under any other
    // handle belongs to a different share and is not this link's to open.
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut files = Vec::new();
    for node in &nodes {
        parents.insert(node.handle.clone(), node.parent.clone());
        let Some(raw) = node
            .key_under(&root.handle)
            .and_then(crypto::b64_decode)
            .and_then(|raw| crypto::decrypt_node_key(share, &raw))
        else {
            continue;
        };
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
            if files.len() >= MAX_FILES {
                return Err(Refusal::TooMany);
            }
            files.push((node.handle.clone(), name, node.size));
        }
    }
    let found: Vec<Found> = files
        .into_iter()
        .map(|(handle, name, size)| Found {
            path: path_of(&handle, &root.handle, &parents, &names),
            node: handle,
            name,
            size,
        })
        .collect();
    if found.is_empty() {
        return Err(Refusal::Empty);
    }
    Ok(found)
}

/// The folder's own name followed by the path down to one node, slash separated.
fn path_of(
    node: &str,
    root: &str,
    parents: &BTreeMap<String, String>,
    names: &BTreeMap<String, String>,
) -> String {
    let mut segments = Vec::new();
    let mut current = parents.get(node).map(String::as_str).unwrap_or_default();
    for _ in 0..MAX_DEPTH {
        let Some(name) = names.get(current) else {
            break;
        };
        segments.push(name.as_str());
        if current == root {
            break;
        }
        current = parents.get(current).map(String::as_str).unwrap_or_default();
    }
    segments.reverse();
    segments.join("/")
}

/// A name that is a name: no separators, no control characters, not empty.
#[must_use]
pub fn clean(name: Option<&str>) -> Option<String> {
    let cleaned: String = name?
        .chars()
        .map(|character| match character {
            '/' | '\\' => '_',
            other if other.is_control() => ' ',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod walk_tests;
