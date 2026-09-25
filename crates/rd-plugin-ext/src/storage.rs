//! Upload destination plugins (RD-090-17).
//!
//! The half that knows about WebAssembly. `rd-extract` sees only the trait it defines, the
//! same arrangement the post-processing steps use, so its upload step stays testable without
//! a runtime and the commit-before-delete rule stays in one place: here.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use rd_extract::{StorageUpload, StorageUploader, UploadProgress, UploadReport};
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{SourceState, StoragePlugin, Upload, UploadOutcome},
};

/// The installed upload destinations, newest version of each.
pub struct StorageDestinations {
    plugins: HashMap<String, Destination>,
}

struct Destination {
    manifest: PluginManifest,
    plugin: StoragePlugin,
}

/// What a destination looks like to whoever is choosing one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadDestinationInfo {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
}

impl StorageDestinations {
    /// Loads every installed upload destination, skipping any that fails to build.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let loaded = registry.instantiate(&PluginType::Storage, |package| {
            StoragePlugin::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Destination {
                    manifest: package.manifest.clone(),
                    plugin,
                },
            )
        });
        let mut plugins = HashMap::new();
        for destination in loaded {
            plugins
                .entry(destination.manifest.id.to_string())
                .or_insert(destination);
        }
        Self { plugins }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Every installed destination, sorted by name.
    #[must_use]
    pub fn list(&self) -> Vec<UploadDestinationInfo> {
        let mut destinations: Vec<UploadDestinationInfo> = self
            .plugins
            .values()
            .map(|destination| UploadDestinationInfo {
                plugin_id: destination.manifest.id.to_string(),
                name: destination.manifest.name.clone(),
                version: destination.manifest.version.clone(),
            })
            .collect();
        destinations.sort_by(|left, right| left.name.cmp(&right.name));
        destinations
    }
}

#[async_trait]
impl StorageUploader for StorageDestinations {
    fn installed(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    async fn upload(&self, plugin_id: &str, upload: StorageUpload<'_>) -> Result<UploadReport> {
        let Some(destination) = self.plugins.get(plugin_id) else {
            anyhow::bail!("no installed upload destination with id {plugin_id}");
        };
        let mut uploaded = Vec::new();
        // Every size before the first byte moves. A plugin reports progress within the file
        // it is sending, and a display fed that number alone would walk back to zero at every
        // file boundary; what the person is waiting for is the package.
        let mut sizes = Vec::with_capacity(upload.files.len());
        for file in upload.files {
            sizes.push(
                tokio::fs::metadata(upload.directory.join(file))
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or_default(),
            );
        }
        let package = sizes.iter().sum::<u64>();
        let mut sent = 0_u64;
        for (file, size) in upload.files.iter().zip(sizes) {
            let source = SourceState::new(
                upload.handle.to_owned(),
                upload.directory.to_path_buf(),
                upload.files.to_vec(),
            )
            .with_progress(package_progress(
                Arc::clone(&upload.progress),
                sent,
                package,
            ));
            let outcome = destination
                .plugin
                .put(
                    source,
                    Upload {
                        file_name: file,
                        size,
                        destination: upload.destination,
                        username: upload.username,
                        secret_ref: upload.secret_ref,
                        checkpoint: None,
                    },
                )
                .await?;
            match outcome {
                UploadOutcome::Complete { remote_id } => {
                    // Commit before delete: the destination is asked, separately, whether it
                    // really holds what it just said it accepted. Without this a server that
                    // answered 201 and stored nothing would take the only copy with it — the
                    // exact gap `rclone move` leaves open, and the reason `verify` exists.
                    let Some(remote_id) = remote_id else {
                        return Ok(UploadReport::Failed {
                            message: format!("{file}: the destination named nothing to verify"),
                        });
                    };
                    match destination
                        .plugin
                        .verify(upload.handle, &remote_id, upload.secret_ref)
                        .await
                    {
                        Ok(true) => uploaded.push(file.clone()),
                        Ok(false) => {
                            return Ok(UploadReport::Failed {
                                message: format!("{file}: the destination does not hold it"),
                            });
                        }
                        Err(error) => {
                            return Ok(UploadReport::Failed {
                                message: format!("{file}: {error}"),
                            });
                        }
                    }
                }
                UploadOutcome::Stopped { .. } => return Ok(UploadReport::Stopped),
                UploadOutcome::Failed { message } => {
                    return Ok(UploadReport::Failed {
                        message: format!("{file}: {message}"),
                    });
                }
            }
            sent += size;
        }
        Ok(UploadReport::Verified { files: uploaded })
    }
}

/// Turns one file's progress into the package's, for the caller that started the upload.
///
/// `before` is what earlier files already contributed and `package` the size of all of them;
/// the guest's own total is dropped, because it describes the file and the caller is showing
/// the package. A package of no measurable size reports no total rather than a made-up one.
fn package_progress(
    progress: UploadProgress,
    before: u64,
    package: u64,
) -> impl Fn(u64, Option<u64>) + Send + Sync + 'static {
    move |done, _file| {
        progress(
            before.saturating_add(done),
            (package > 0).then_some(package),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{Arc, UploadProgress, package_progress};

    /// What the caller's progress closure was handed, in order.
    type Seen = Arc<Mutex<Vec<(u64, Option<u64>)>>>;

    #[test]
    fn the_second_file_continues_where_the_first_one_stopped() {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let progress: UploadProgress =
            Arc::new(move |done, total| recorded.lock().expect("seen").push((done, total)));

        // Two files of 100 bytes. The second one reporting 40 of its own bytes is 140 of the
        // package, not 40 of it again.
        package_progress(Arc::clone(&progress), 0, 200)(100, Some(100));
        package_progress(Arc::clone(&progress), 100, 200)(40, Some(100));

        assert_eq!(
            *seen.lock().expect("seen"),
            [(100, Some(200)), (140, Some(200))]
        );
    }

    #[test]
    fn a_package_of_unknown_size_reports_no_total() {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let progress: UploadProgress =
            Arc::new(move |done, total| recorded.lock().expect("seen").push((done, total)));

        package_progress(progress, 0, 0)(7, Some(7));

        assert_eq!(*seen.lock().expect("seen"), [(7, None)]);
    }
}
