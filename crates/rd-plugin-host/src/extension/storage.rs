//! Storage destinations (RD-090-17): upload, then confirm before anything is deleted.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, SourceState, bindings::storage};
use crate::{PluginManifest, runtime::PluginStoreState};

/// How an upload ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UploadOutcome {
    /// Uploaded; the payload is the destination's own identifier for the object.
    Complete {
        remote_id: Option<String>,
    },
    Stopped {
        checkpoint: Vec<u8>,
    },
    Failed {
        message: String,
    },
}

/// A compiled storage destination.
pub struct StoragePlugin {
    runtime: ExtensionRuntime,
    pre: storage::StoragePluginPre<PluginStoreState>,
}

impl StoragePlugin {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: storage::StoragePluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Uploads one file of a package.
    pub async fn put(&self, source: SourceState, upload: Upload<'_>) -> Result<UploadOutcome> {
        let handle = source.handle().to_owned();
        // Narrowed to the host this upload is for. The manifest says `*` because where a
        // person put their server is not knowable in advance; this is the grant that applies.
        let mut store = self.runtime.source_store(
            None,
            upload.secret_ref.map(str::to_owned),
            host_of(upload.destination).as_deref(),
            source,
        )?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let job = storage::exports::rdownloader::plugin::storage::UploadJob {
            handle,
            file_name: upload.file_name.to_owned(),
            size: upload.size,
            destination: upload.destination.to_owned(),
            username: upload.username.map(str::to_owned),
            credential_ref: upload.secret_ref.map(str::to_owned),
            checkpoint: upload.checkpoint,
        };
        let end = instance
            .rdownloader_plugin_storage()
            .call_put(&mut store, &job)
            .await?;
        use storage::exports::rdownloader::plugin::storage::UploadEnd;
        Ok(match end {
            UploadEnd::Complete(remote_id) => UploadOutcome::Complete { remote_id },
            UploadEnd::Stopped(checkpoint) => UploadOutcome::Stopped { checkpoint },
            UploadEnd::Failed(failure) => UploadOutcome::Failed {
                message: failure.message,
            },
        })
    }

    /// Asks the destination to confirm it holds the object.
    ///
    /// The host deletes nothing until this says yes: commit before delete is the whole
    /// reason this call exists separately from `put`.
    pub async fn verify(
        &self,
        handle: &str,
        remote_id: &str,
        secret_ref: Option<&str>,
    ) -> Result<bool> {
        let mut store = self.runtime.store_for(
            None,
            secret_ref.map(str::to_owned),
            host_of(remote_id).as_deref(),
        )?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        instance
            .rdownloader_plugin_storage()
            .call_verify(&mut store, handle, remote_id)
            .await?
            .map_err(|failure| anyhow::anyhow!("{}", failure.message))
    }
}

/// The host part of a configured destination, if it is a URL at all.
///
/// `None` leaves the manifest's list in charge, which for a destination that is not a URL is
/// the honest answer: there is no host to narrow to.
fn host_of(destination: &str) -> Option<String> {
    url::Url::parse(destination)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// One file on its way to one destination.
///
/// A struct rather than six positional arguments: most of them are strings, and a caller that
/// swapped two would upload the right bytes to the wrong place.
pub struct Upload<'a> {
    pub file_name: &'a str,
    pub size: u64,
    /// Where this destination writes, as configured. Never a secret.
    pub destination: &'a str,
    /// The stored login's user name, if it has one.
    pub username: Option<&'a str>,
    /// The vault reference the plugin's `{{secret}}` expands to.
    pub secret_ref: Option<&'a str>,
    pub checkpoint: Option<Vec<u8>>,
}
