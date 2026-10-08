//! Turning a prefix into the reviewable [`RemoteListing`].
//!
//! A bucket has no directories, only keys with slashes in them, so the folders of the review
//! tree are derived from the keys. The walk is bounded like the FTP and SFTP ones: a prefix
//! with a million objects produces a listing nobody can review, and hitting the limit is
//! recorded rather than silently cut. The limit counts every object the service hands over,
//! the ones skipped as unsafe or too deep included (RD-1190-20): a bucket of a million unsafe
//! keys ends the walk as early as one of a million good ones.

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
    let mut seen = 0_usize;
    while let Some(item) = objects.next().await {
        if seen >= MAX_REMOTE_ENTRIES || files.len() + folders.len() >= MAX_REMOTE_ENTRIES {
            truncated = Some(ListingLimit::EntryCount);
            break;
        }
        seen += 1;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::{ObjectStore, PutOptions, PutPayload, memory::InMemory, path::Path};
    use rd_core::{ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, ObjectAddress};

    use super::walk;
    use crate::connect::Store;

    /// RD-1190-20: keys the walk skips count against the limit too. Before, a prefix of
    /// nothing but too-deep keys was walked to its end however many there were, and came back
    /// marked as cut for depth only.
    #[tokio::test]
    async fn skipped_keys_count_against_the_listing_limit() {
        let memory = Arc::new(InMemory::new());
        let deep = vec!["d"; MAX_REMOTE_DEPTH].join("/");
        for index in 0..=MAX_REMOTE_ENTRIES {
            memory
                .put_opts(
                    &Path::from(format!("deep/{deep}/{index}.bin")),
                    PutPayload::from_static(b"x"),
                    PutOptions::default(),
                )
                .await
                .expect("put");
        }
        let store = Store {
            objects: memory.clone(),
            parts: memory,
        };
        let address =
            ObjectAddress::parse(&url::Url::parse("s3://media-bucket/deep/").expect("url"))
                .expect("address");
        let listing = walk(&store, &address).await.expect("listing");
        assert!(listing.entries.is_empty());
        assert_eq!(listing.truncated, Some(ListingLimit::EntryCount));
    }
}
