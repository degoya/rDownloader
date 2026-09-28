//! Turning a prefix into the reviewable [`RemoteListing`].
//!
//! A bucket has no directories, only keys with slashes in them, so the folders of the review
//! tree are derived from the keys. The walk is bounded like the FTP and SFTP ones: a prefix
//! with a million objects produces a listing nobody can review, and hitting the limit is
//! recorded rather than silently cut.

use std::collections::BTreeSet;

use futures_util::StreamExt;
use object_store::path::Path;
use rd_core::{
    ByteCount, Failure, ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, ObjectAddress,
    RemoteEntry, RemoteListing, is_safe_relative_path,
};

use crate::{connect::Store, error};

/// Lists every object below the address's prefix.
pub(crate) async fn walk(store: &Store, address: &ObjectAddress) -> Result<RemoteListing, Failure> {
    let prefix = address.key.trim_end_matches('/');
    let location = Path::parse(prefix).map_err(|_| error::address_invalid())?;
    let scope = (!prefix.is_empty()).then_some(&location);
    let mut objects = store.objects.list(scope);
    let mut files = Vec::new();
    let mut folders = BTreeSet::new();
    let mut truncated = None;
    while let Some(item) = objects.next().await {
        let meta = item.map_err(|error| error::classify(&error, &address.bucket))?;
        let key: &str = meta.location.as_ref();
        let relative = key
            .strip_prefix(prefix)
            .unwrap_or(key)
            .trim_start_matches('/');
        if !is_safe_relative_path(relative) {
            tracing::warn!("skipping an object whose key cannot be stored safely");
            continue;
        }
        if relative.split('/').count() > MAX_REMOTE_DEPTH {
            truncated = Some(ListingLimit::Depth);
            continue;
        }
        if files.len() + folders.len() >= MAX_REMOTE_ENTRIES {
            truncated = Some(ListingLimit::EntryCount);
            break;
        }
        // Every parent of the key is a folder of the review tree.
        let mut end = 0;
        while let Some(offset) = relative[end..].find('/') {
            end += offset;
            folders.insert(relative[..end].to_owned());
            end += 1;
        }
        files.push(RemoteEntry {
            path: relative.to_owned(),
            is_dir: false,
            size: ByteCount::new(meta.size).ok(),
            modified: Some(meta.last_modified),
            etag: meta.e_tag.clone(),
        });
    }
    let mut entries: Vec<RemoteEntry> = folders
        .into_iter()
        .map(|path| RemoteEntry {
            path,
            is_dir: true,
            size: None,
            modified: None,
            etag: None,
        })
        .collect();
    entries.extend(files);
    Ok(RemoteListing {
        root: format!("/{}/{prefix}", address.bucket)
            .trim_end_matches('/')
            .to_owned(),
        single_file: false,
        entries,
        truncated,
        // Ranged reads are part of the protocol; every object can be resumed.
        supports_resume: true,
    })
}
