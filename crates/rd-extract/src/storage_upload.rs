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
    /// File names relative to the package directory.
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
fn reporter(
    inner: &crate::Inner,
    owner: &str,
    destination: &str,
) -> (UploadProgress, tokio::task::JoinHandle<()>) {
    use rd_core::{PostprocessKind, PostprocessStage};

    let (sender, mut numbers) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
    let database = inner.database.clone();
    let owner = owner.to_owned();
    let destination = destination.to_owned();
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
                    Some(destination.clone()),
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
/// Returns `false` on failure, exactly as the rclone path does, so the caller does not have to
/// know which one ran.
pub(crate) async fn run(
    inner: &crate::Inner,
    owner: &str,
    remote: PluginRemote<'_>,
    package_name: &str,
    directory: &Path,
    files: &[String],
    mode: crate::rclone_job::UploadMode,
) -> Result<bool> {
    use rd_core::{PostprocessKind, PostprocessStage, PostprocessState};

    let Some(uploader) = inner.storage.as_ref() else {
        // Planned but not runnable: the destination was installed when the package was
        // planned and is gone now. Saying so beats an upload that silently did not happen.
        crate::steps::checkpoint(
            inner,
            owner,
            PostprocessKind::Upload,
            remote.destination,
            PostprocessState::Failed,
            None,
            Some("no upload destination plugins are loaded".to_owned()),
        )
        .await?;
        return Ok(false);
    };
    crate::steps::stage(
        inner,
        owner,
        PostprocessStage::Uploading,
        Some(remote.destination.to_owned()),
    )
    .await?;
    crate::steps::checkpoint(
        inner,
        owner,
        PostprocessKind::Upload,
        remote.destination,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    // The login for this address, if one is configured. The plugin gets the user name and an
    // opaque handle; the password stays in the vault and reaches the request through the host.
    let credential = inner.remote_login(remote.destination).await;
    let (progress, written) = reporter(inner, owner, remote.destination);
    let report = uploader
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
            },
        )
        .await;
    // The upload owns the only other handle on the sender, so it is closed by now and this
    // waits for the last number to be written rather than for anything to happen.
    let _ = written.await;
    let (state, message, ok) = match report {
        Ok(UploadReport::Verified { files: uploaded }) => {
            let mut note = format!("{} files uploaded and verified", uploaded.len());
            if mode == crate::rclone_job::UploadMode::Move {
                // Only here, and only for the files the destination confirmed it holds. This
                // is the whole point of `verify` being a call of its own: a server that
                // answered 201 and stored nothing would otherwise take the only copy with it.
                let removed = remove_local(directory, &uploaded).await;
                note.push_str(&format!(", {removed} removed locally"));
            }
            (PostprocessState::Completed, Some(note), true)
        }
        // Not a failure: what arrived stays, and the next run continues. The step goes back to
        // queued rather than being marked done on a job that is not.
        Ok(UploadReport::Stopped) => (PostprocessState::Queued, None, true),
        Ok(UploadReport::Failed { message }) => (PostprocessState::Failed, Some(message), false),
        Err(error) => (PostprocessState::Failed, Some(error.to_string()), false),
    };
    crate::steps::checkpoint(
        inner,
        owner,
        PostprocessKind::Upload,
        remote.destination,
        state,
        None,
        message.map(crate::steps::truncate),
    )
    .await?;
    // `package_name` shapes the remote folder, which the plugin derives from the handle it was
    // given; nothing local is named after it here.
    let _ = package_name;
    Ok(ok)
}

/// Deletes the local copies of files the destination confirmed it holds.
///
/// A file that will not delete is reported by count and nothing more: the upload succeeded,
/// and failing the package because a leftover could not be removed would be the wrong end of
/// the trade. The package directory itself is left alone — something else may still be in it.
async fn remove_local(directory: &Path, files: &[String]) -> usize {
    let mut removed = 0;
    for file in files {
        let path = directory.join(file);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => removed += 1,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "uploaded file could not be removed locally");
            }
        }
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
