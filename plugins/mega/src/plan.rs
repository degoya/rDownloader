//! What this plugin answers with, given what MEGA said.
//!
//! Outside the component so it compiles, and is tested, on the host target; `guest.rs` is the
//! translation of these values into the WIT vocabulary and nothing else. Nothing here reaches
//! the network, and nothing here decrypts a payload byte -- the description is the product.

use mega_common::{
    api, chunks,
    crypto::{self, Attributes, FileKey},
};
use serde_json::Value;

use crate::messages;

/// How the scheduler should treat a refusal. The guest maps these onto `failure-kind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Category {
    /// Not this plugin's address after all; the host asks whoever else claims it.
    Unsupported,
    Permanent,
    Transient,
    RateLimited,
}

/// A refusal: the code and text pair from [`messages`], plus how to treat it.
pub type Refusal = ((&'static str, &'static str), Category);

/// Everything the host is told about one file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Description {
    /// The storage address. Short-lived by design: MEGA answers a different path every call,
    /// and the scheduler asks again before it resumes.
    pub url: String,
    pub file_name: Option<String>,
    pub size: u64,
    /// The 16-byte payload key. It leaves this crate once, into the host's vault type.
    pub key: Vec<u8>,
    pub nonce: Vec<u8>,
    /// Absent for a file whose key carried no condensed value -- a node inside a folder
    /// always has one, so in practice only a malformed key gets here.
    pub integrity: Option<Integrity>,
}

/// The chunk layout and the value the plaintext has to condense to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Integrity {
    pub boundaries: Vec<u64>,
    pub iv: Vec<u8>,
    pub expected: Vec<u8>,
}

/// Reads the 32-byte key a file address carries in its fragment.
pub fn file_key(fragment: &str) -> Result<FileKey, Refusal> {
    crypto::b64_decode(fragment)
        .as_deref()
        .and_then(FileKey::from_raw)
        .ok_or((messages::KEY_INVALID, Category::Permanent))
}

/// Reads the 16-byte share key a folder address carries.
pub fn share_key(fragment: &str) -> Result<[u8; 16], Refusal> {
    crypto::b64_decode(fragment)
        .and_then(|raw| <[u8; 16]>::try_from(raw).ok())
        .ok_or((messages::KEY_INVALID, Category::Permanent))
}

/// The key of one node of a folder listing, opened with the share key.
///
/// Only the entry filed under the share root is encrypted with the key from the fragment; an
/// entry under the node's own handle belongs to a different share and is not ours to open.
pub fn node_key(listing: &Value, node: &str, share: &[u8; 16]) -> Result<FileKey, Refusal> {
    let nodes = api::Node::list(listing);
    let root = api::share_root(&nodes)
        .ok_or((messages::INVALID_RESPONSE, Category::Permanent))?
        .handle
        .clone();
    let found = nodes
        .iter()
        .find(|entry| entry.handle == node && entry.kind == 0)
        .ok_or((messages::NODE_MISSING, Category::Permanent))?;
    let raw = found
        .key_under(&root)
        .and_then(crypto::b64_decode)
        .and_then(|raw| crypto::decrypt_node_key(share, &raw))
        .ok_or((messages::KEY_INVALID, Category::Permanent))?;
    FileKey::from_raw(&raw).ok_or((messages::KEY_INVALID, Category::Permanent))
}

/// The key of one file of the signed-in account, **still wrapped** under the master key
/// (RD-120-30).
///
/// Only the host can open it: the master key is the key half of the session and never
/// enters this sandbox. What leaves here goes into a single `aes-ecb-decrypt` step over the
/// session, and what comes back is this file's own key and nothing wider.
pub fn account_wrapped_key(listing: &Value, node: &str) -> Result<Vec<u8>, Refusal> {
    let nodes = api::Node::list(listing);
    let found = nodes
        .iter()
        .find(|entry| entry.handle == node && entry.kind == 0)
        .ok_or((messages::ACCOUNT_NODE_MISSING, Category::Permanent))?;
    let wrapped = found
        .own_key()
        .ok_or((messages::ACCOUNT_KEY_FOREIGN, Category::Permanent))
        .and_then(|text| {
            crypto::b64_decode(text).ok_or((messages::KEY_INVALID, Category::Permanent))
        })?;
    // A file's node key is 32 bytes: the key folded with the counter prefix and the condensed
    // integrity value. Anything else is not a file key, and the host is not asked to open it.
    if wrapped.len() != 32 {
        return Err((messages::KEY_INVALID, Category::Permanent));
    }
    Ok(wrapped)
}

/// Turns an `a=g` answer and the file's key into the description the host computes with.
pub fn describe(answer: &Value, key: &FileKey) -> Result<Description, Refusal> {
    let (url, size, attributes) =
        api::download_target(answer).ok_or((messages::INVALID_RESPONSE, Category::Permanent))?;
    // The attribute block is the proof that this key belongs to this file: it decrypts to a
    // `MEGA` prefix under the right key and to noise under any other. Checking it here means
    // a wrong key is a refusal before a byte is fetched rather than an integrity mismatch
    // after the whole file has been.
    let file_name = if attributes.is_empty() {
        None
    } else {
        let plain = crypto::b64_decode(&attributes)
            .as_deref()
            .and_then(|blob| crypto::decrypt_attributes(&key.key, blob))
            .ok_or((messages::ATTRIBUTES_UNREADABLE, Category::Permanent))?;
        clean_name(Attributes::parse(&plain).name.as_deref())
    };
    let boundaries = chunks::boundaries(size);
    let integrity = (!boundaries.is_empty()).then(|| Integrity {
        boundaries,
        iv: key.mac_iv().to_vec(),
        expected: key.meta_mac.to_vec(),
    });
    Ok(Description {
        url,
        file_name,
        size,
        key: key.key.to_vec(),
        nonce: key.nonce.to_vec(),
        integrity,
    })
}

/// A file name that is a name: no separators, no control characters, not empty.
///
/// The value comes out of somebody else's attribute block, and the staging path is built from
/// it. `rd-files` has the last word, but a plugin that hands on `../` has already failed.
#[must_use]
pub fn clean_name(name: Option<&str>) -> Option<String> {
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

/// How one of MEGA's negative answers is reported.
///
/// `None` is a number this plugin has no name for; the caller reports it as `api_error` with
/// the number as a parameter rather than inventing a meaning for it.
#[must_use]
pub fn api_refusal(code: i64) -> Option<Refusal> {
    Some(match code {
        api::ENOENT => (messages::NOT_FOUND, Category::Permanent),
        api::EACCESS => (messages::ACCESS_DENIED, Category::Permanent),
        api::ESID => (messages::SESSION_REQUIRED, Category::Permanent),
        api::EAGAIN | api::ERATELIMIT => (messages::RATE_LIMITED, Category::RateLimited),
        api::EOVERQUOTA => (messages::QUOTA_EXCEEDED, Category::RateLimited),
        api::EBLOCKED | api::ETEMPUNAVAIL => (messages::UNAVAILABLE, Category::Transient),
        _ => return None,
    })
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod plan_tests;
