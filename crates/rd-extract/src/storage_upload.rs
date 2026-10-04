//! Uploading a finished package through a plugin destination (RD-090-17).
//!
//! The counterpart of [`crate::rclone_job`]. Both end at the same step in the pipeline; which
//! one runs is decided by the configured remote — `plugin:<id>/<destination>` picks this one.
//!
//! The rule this file exists to keep is **commit before delete**: local files go only after
//! the destination has confirmed, separately from the upload, that it holds them. `rclone
//! move` cannot offer that, which is why the rclone path still copies-then-trusts and this
//! one does not.

use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;

/// How far an upload has got, as the destination plugin itself reports it.
///
/// `done` counts the bytes of the whole package the plugin has handed over; `total` is the
/// package's size when it is known. Called from inside the guest invocation, so it has to
/// return at once — writing the number somewhere is the receiver's problem, not the caller's.
pub type UploadProgress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// One package on its way to one destination.
pub struct StorageUpload<'a> {
    /// Package-scoped handle. Not a path: the plugin names files, never locations.
    pub handle: &'a str,
    pub directory: &'a Path,
    /// File paths relative to the package directory, `/` between folders (`Film/film.mkv`).
    pub files: &'a [String],
    /// Where this destination writes, as configured. Never a secret.
    pub destination: &'a str,
    /// The stored login's user name, if the destination has one.
    pub username: Option<&'a str>,
    /// The vault reference the plugin's secret marker expands to.
    pub secret_ref: Option<&'a str>,
    /// Where the plugin's own progress goes.
    ///
    /// A field rather than an `Option`, and no default: a plugin that reports progress into
    /// nothing is what RD-108-18 was about, and a caller that has nowhere to show it should
    /// have to write that down.
    pub progress: UploadProgress,
    /// The upload limit the plugin's reads of the package are paced by (RD-150-15).
    pub bandwidth: rd_limits::ScopedLimiter,
}

/// What an upload attempt achieved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UploadReport {
    /// Every file arrived **and** the destination confirmed it holds each one.
    Verified {
        files: Vec<String>,
    },
    /// Interrupted; whatever arrived stays, and the next attempt continues.
    Stopped,
    Failed {
        message: String,
    },
}

/// The installed upload destinations, as this crate needs to see them.
#[async_trait]
pub trait StorageUploader: Send + Sync {
    /// Whether a plugin id names an installed destination.
    fn installed(&self, plugin_id: &str) -> bool;

    /// Uploads one package and verifies what arrived.
    async fn upload(&self, plugin_id: &str, upload: StorageUpload<'_>) -> Result<UploadReport>;
}

/// Where a destination plugin's progress goes: the same row the rclone path writes to.
///
/// Two hops rather than one. The host calls the reporter from inside the guest invocation,
/// which is synchronous and must not block, while writing to the database is neither — so the
/// closure only hands the number to a task that writes it at the pace the display needs. The
/// task ends by itself when the upload drops the sender.
pub(crate) fn reporter(
    inner: &crate::Inner,
    owner: &str,
    destination: &str,
    shown: &str,
) -> (UploadProgress, tokio::task::JoinHandle<()>) {
    use rd_core::{PostprocessKind, PostprocessStage};

    let (sender, mut numbers) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
    let database = inner.database.clone();
    let owner = owner.to_owned();
    let destination = destination.to_owned();
    let shown = shown.to_owned();
    let written = tokio::spawn(async move {
        let mut last = std::time::Instant::now() - crate::rclone_job::PROGRESS_INTERVAL;
        while let Some((done, total)) = numbers.recv().await {
            // The same throttle the rclone path uses: a plugin may report per chunk, and the
            // display is not worth a database write per chunk.
            if last.elapsed() < crate::rclone_job::PROGRESS_INTERVAL {
                continue;
            }
            last = std::time::Instant::now();
            let _ = database
                .postprocess_progress(
                    owner.clone(),
                    PostprocessKind::Upload,
                    destination.clone(),
                    PostprocessStage::Uploading,
                    crate::rclone_job::percent(done, total),
                    Some(shown.clone()),
                )
                .await;
        }
    });
    (
        Arc::new(move |done, total| {
            // A closed receiver means the upload is over; the number has nowhere to go and
            // nothing to be done about it.
            let _ = sender.send((done, total));
        }),
        written,
    )
}

/// Runs the upload step through a plugin destination.
///
/// Says how the step ended, exactly as the rclone path does, so the caller does not have to
/// know which one ran. `configured` is the remote as the step was planned
/// (`plugin:<id>/<destination>`): every checkpoint names it, or the planned row would stay
/// queued beside a second one and the pipeline would run again at every start (INTAKE-01).
pub(crate) async fn run(
    inner: &crate::Inner,
    owner: &str,
    configured: &str,
    remote: PluginRemote<'_>,
    directory: &Path,
    files: &[String],
    mode: crate::rclone_job::UploadMode,
) -> Result<crate::steps::StepEnd> {
    let upload = inner.storage.as_ref().map(|uploader| {
        move |progress: UploadProgress| async move {
            // The login for this address, if one is configured. The plugin gets the user name
            // and an opaque handle; the password stays in the vault and reaches the request
            // through the host.
            let credential = inner.remote_login(remote.destination).await;
            uploader
                .upload(
                    remote.plugin_id,
                    StorageUpload {
                        handle: owner,
                        directory,
                        files,
                        destination: remote.destination,
                        username: credential
                            .as_ref()
                            .and_then(|login| login.username.as_deref()),
                        secret_ref: credential
                            .as_ref()
                            .and_then(|login| login.secret_ref.as_deref()),
                        progress,
                        bandwidth: inner.upload_limit(),
                    },
                )
                .await
        }
    });
    crate::upload_step::run(
        inner,
        crate::upload_step::UploadStep {
            owner,
            label: configured,
            shown: remote.destination,
            directory,
            mode,
            // Planned but not runnable: the destination was installed when the package was
            // planned and is gone now.
            unavailable: "no upload destination plugins are loaded",
        },
        upload,
    )
    .await
}

/// Deletes the local copies of files the destination confirmed it holds, then the folders
/// that deleting them emptied.
///
/// A file that will not delete is reported by count and nothing more: the upload succeeded,
/// and failing the package because a leftover could not be removed would be the wrong end of
/// the trade. A folder goes only once it is empty — one that still holds anything, a file that
/// would not delete or one that was never offered, stays with it — and only a folder one of
/// these files was in. The package directory itself is left alone — something else may still
/// be in it.
pub(crate) async fn remove_local(directory: &Path, files: &[String]) -> usize {
    let mut removed = 0;
    let mut folders = std::collections::BTreeSet::new();
    for file in files {
        let path = directory.join(file);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                removed += 1;
                let mut parent = Path::new(file).parent();
                while let Some(folder) = parent.filter(|folder| !folder.as_os_str().is_empty()) {
                    folders.insert(folder.to_path_buf());
                    parent = folder.parent();
                }
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "uploaded file could not be removed locally");
            }
        }
    }
    // Deepest first, so a folder is tried after everything below it. `remove_dir` refuses a
    // folder that is not empty, which is the whole check.
    let mut folders: Vec<_> = folders.into_iter().collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    for folder in folders {
        let _ = tokio::fs::remove_dir(directory.join(folder)).await;
    }
    removed
}

/// The plugin and destination named by a `plugin:<id>/<destination>` remote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PluginRemote<'a> {
    pub plugin_id: &'a str,
    pub destination: &'a str,
}

/// Reads a configured remote as a plugin destination, or `None` when it is an rclone one.
///
/// The form is `plugin:<plugin-id>/<destination>`. Everything after the first slash is the
/// destination and is passed to the plugin verbatim — it is that plugin's own vocabulary, and
/// the core has no business interpreting it.
pub(crate) fn parse_plugin_remote(remote: &str) -> Option<PluginRemote<'_>> {
    let rest = remote.strip_prefix("plugin:")?;
    let (plugin_id, destination) = rest.split_once('/')?;
    let plugin_id = plugin_id.trim();
    let destination = destination.trim();
    (!plugin_id.is_empty() && !destination.is_empty()).then_some(PluginRemote {
        plugin_id,
        destination,
    })
}

#[cfg(test)]
mod tests {
    use super::{PluginRemote, parse_plugin_remote};

    #[test]
    fn a_plugin_remote_is_split_at_its_first_slash_only() {
        // The destination is a WebDAV address, slashes and all. Splitting it further would
        // mean interpreting a vocabulary that belongs to the plugin.
        assert_eq!(
            parse_plugin_remote(
                "plugin:019d0000-0000-7000-8000-000000000108/https://cloud.example/dav/Downloads"
            ),
            Some(PluginRemote {
                plugin_id: "019d0000-0000-7000-8000-000000000108",
                destination: "https://cloud.example/dav/Downloads",
            })
        );
    }

    #[test]
    fn an_rclone_remote_is_left_alone() {
        assert_eq!(parse_plugin_remote("gdrive:downloads"), None);
        assert_eq!(parse_plugin_remote("archive:movies/2026"), None);
    }

    #[test]
    fn a_half_written_plugin_remote_is_not_one() {
        // Better to fall through to rclone, which reports a remote it does not know, than to
        // dispatch to a plugin with nowhere to put anything.
        assert_eq!(parse_plugin_remote("plugin:"), None);
        assert_eq!(parse_plugin_remote("plugin:only-an-id"), None);
        assert_eq!(parse_plugin_remote("plugin:/no-id"), None);
    }
}
