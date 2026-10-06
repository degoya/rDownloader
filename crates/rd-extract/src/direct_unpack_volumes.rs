//! The volumes a direct unpack waits for: whether the next one is ready, still arriving, or
//! will not come, judged from the package's downloads.

use std::{path::Path, time::Duration};

use anyhow::Result;
use rd_core::{DownloadFile, DownloadState, EventKind, PackageId};

use crate::Inner;

/// How often a waiting unpack looks at its volume even without an event: a lagged or closed
/// event bus must not leave it waiting for good.
const RECHECK: Duration = Duration::from_secs(10);

/// What a volume the tool waits for is doing.
#[derive(Debug, Eq, PartialEq)]
enum VolumeState {
    /// Completed and on disk.
    Ready,
    /// Still on its way, or not known by its name yet.
    Waiting,
    /// It will not come intact, or not soon: the unpack gives up.
    GivenUp(&'static str),
}

/// Waits until `volume` is ready (`true`) or will not be (`false`).
pub(super) async fn await_volume(
    inner: &Inner,
    package_id: PackageId,
    directory: &Path,
    volume: &str,
) -> bool {
    let mut events = inner.database.subscribe();
    loop {
        match volume_state(inner, package_id, directory, volume).await {
            Ok(VolumeState::Ready) => return true,
            Ok(VolumeState::Waiting) => {}
            Ok(VolumeState::GivenUp(reason)) => {
                tracing::info!(%package_id, volume, reason, "direct unpack stops before a volume");
                return false;
            }
            Err(error) => {
                tracing::warn!(%package_id, volume, %error, "direct unpack could not look at a volume");
                return false;
            }
        }
        // Any change of a download's state may be this volume's; a periodic look covers a
        // lagged or closed bus.
        let deadline = tokio::time::Instant::now() + RECHECK;
        loop {
            tokio::select! {
                () = inner.shutdown.cancelled() => return false,
                () = tokio::time::sleep_until(deadline) => break,
                event = events.recv() => match event {
                    Ok(event) if event.kind == EventKind::DownloadState => break,
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep_until(deadline).await;
                        break;
                    }
                },
            }
        }
    }
}

async fn volume_state(
    inner: &Inner,
    package_id: PackageId,
    directory: &Path,
    volume: &str,
) -> Result<VolumeState> {
    let downloads = inner.database.downloads_for_package(package_id).await?;
    Ok(state_of(&downloads, directory, volume))
}

/// The verdict on `volume` from the package's downloads.
///
/// A name the package does not know yet is not a missing volume while files are still arriving:
/// an obfuscated post is only named once its file is assembled.
fn state_of(downloads: &[DownloadFile], directory: &Path, volume: &str) -> VolumeState {
    let wanted = downloads
        .iter()
        .find(|file| file.file_name == volume)
        .or_else(|| {
            downloads
                .iter()
                .find(|file| file.file_name.eq_ignore_ascii_case(volume))
        });
    match wanted {
        Some(file) if file.state == DownloadState::Completed => {
            if directory.join(&file.file_name).is_file() {
                VolumeState::Ready
            } else {
                VolumeState::GivenUp("the volume is not on disk")
            }
        }
        Some(file) if arriving(file) => VolumeState::Waiting,
        Some(file) if missing_articles(file) => {
            VolumeState::GivenUp("the volume is missing articles")
        }
        Some(file) if file.state == DownloadState::Paused => {
            VolumeState::GivenUp("the volume is paused")
        }
        Some(_) => VolumeState::GivenUp("the volume did not arrive"),
        None if downloads.iter().any(arriving) => VolumeState::Waiting,
        None if downloads
            .iter()
            .any(|file| file.state == DownloadState::Paused) =>
        {
            VolumeState::GivenUp("the package is paused")
        }
        None => VolumeState::GivenUp("the volume is not part of the package"),
    }
}

/// The code a Usenet file carries while it waits, `Verifying`, for the PAR2 verdict on the
/// articles no server had (`rd_db::nzb_queue::AWAITING_PAR2`, RD-108-24).
const MISSING_ARTICLES: &str = "usenet.segments_missing_awaiting_par2";

/// A file that came without some of its articles; only a repair can complete it.
fn missing_articles(file: &DownloadFile) -> bool {
    file.state == DownloadState::Verifying
        && file
            .last_error
            .as_ref()
            .and_then(|failure| failure.code.as_deref())
            == Some(MISSING_ARTICLES)
}

/// A download that is still on its way.
///
/// `Verifying` without missing articles is the moment between a Usenet file's last article
/// and its completion, which every complete file passes through.
pub(super) fn arriving(file: &DownloadFile) -> bool {
    match file.state {
        DownloadState::Queued
        | DownloadState::Resolving
        | DownloadState::Downloading
        | DownloadState::RetryWait => true,
        DownloadState::Verifying => !missing_articles(file),
        _ => false,
    }
}
