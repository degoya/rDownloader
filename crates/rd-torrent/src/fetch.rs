//! The `.torrent` behind an http(s) address, fetched by rDownloader itself (RD-1320-05).
//!
//! librqbit's `AddTorrent::from_url` fetches with a client of its own: no authentication
//! profile, no proxy, no custom CA, no address rule. A link the check read keeps that copy
//! (RD-130-18), but one queued without a check — or after the kept copy expired — went to
//! the tracker around all of it: without the session cookie a private tracker wants, and past
//! the proxy that may be the only way out. The runner fetches the file through the client the
//! row's own HTTP transfer would use and hands librqbit the bytes.
//!
//! The copy is stored by info hash where an uploaded torrent lives, so a retry, a seed re-added
//! after a restart and a recheck take it from there: the tracker is asked once, and a seed
//! that has no queue run to go out on never asks it at all.

use std::path::PathBuf;

use anyhow::Result;
use librqbit::AddTorrent;
use rd_core::{DownloadFile, Failure, FailureKind};
use rd_scheduler::TransferNetwork;

use crate::{MAX_TORRENT_BYTES, TorrentService, parse_torrent};

impl TorrentService {
    /// The engine request for `file`: a magnet or a stored `.torrent` as it is, an http(s)
    /// address as the bytes it answers with, fetched through `network`.
    ///
    /// `network` is `None` where no queue run asks — a seed re-added after a restart — and the
    /// copy fetched before is then the only source: the address is never handed to librqbit.
    pub(crate) async fn request_for(
        &self,
        file: &DownloadFile,
        network: Option<&TransferNetwork>,
    ) -> Result<AddTorrent<'static>, Failure> {
        if !is_http(&file.source) {
            return crate::runner::add_request(&file.source).map_err(|error| {
                Failure::coded(
                    FailureKind::Permanent,
                    "torrent.source_invalid",
                    format!("{error:#}"),
                )
            });
        }
        if let Some(path) = self.stored_copy_path(file).await
            && let Ok(bytes) = tokio::fs::read(&path).await
            && bytes.len() <= MAX_TORRENT_BYTES
        {
            return Ok(AddTorrent::from_bytes(bytes));
        }
        let Some(network) = network else {
            return Err(add_failed(
                "the .torrent file was never fetched, and is fetched only by a queue run",
            ));
        };
        let bytes = fetch(network, &file.source).await.map_err(add_failed)?;
        if let Err(error) = self.store_torrent_file(&bytes).await {
            // Costs a second fetch at the next attempt, nothing more.
            tracing::warn!(download_id = %file.id, %error, "a fetched torrent could not be kept");
        }
        Ok(AddTorrent::from_bytes(bytes))
    }

    /// Where the `.torrent` behind `file` is stored: its `file://` source, or for an http(s)
    /// source the copy fetched for it, by the info hash its run recorded. `None` for a magnet
    /// and for an address whose torrent is not known yet.
    pub(crate) async fn stored_copy_path(&self, file: &DownloadFile) -> Option<PathBuf> {
        if file.source.scheme() == "file" {
            return file.source.to_file_path().ok();
        }
        if !is_http(&file.source) {
            return None;
        }
        let state = self.job_state(file.id).await;
        let info_hash = state.metadata?.info_hash;
        // A hash is hex; anything else is no file name of ours.
        if !info_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit())
        {
            return None;
        }
        Some(
            self.torrent_file_directory()
                .join(format!("{info_hash}.torrent")),
        )
    }
}

fn is_http(source: &url::Url) -> bool {
    matches!(source.scheme(), "http" | "https")
}

/// Fetches and validates the `.torrent` at `url` through the row's own client.
async fn fetch(network: &TransferNetwork, url: &url::Url) -> Result<Vec<u8>, String> {
    let client = network
        .client()
        .await
        .map_err(|error| format!("{error:#}"))?;
    let fetched = rd_http::fetch_document(
        &client.client,
        url.clone(),
        &client.headers,
        MAX_TORRENT_BYTES,
    )
    .await
    .map_err(without_url)?;
    parse_torrent(&fetched.bytes)
        .map_err(|error| format!("the address did not answer with a torrent: {error:#}"))?;
    Ok(fetched.bytes)
}

/// The error without the address: a private tracker's download address carries the passkey.
fn without_url(error: anyhow::Error) -> String {
    match error.downcast::<reqwest::Error>() {
        Ok(error) => error.without_url().to_string(),
        Err(error) => format!("{error:#}"),
    }
}

fn add_failed(reason: impl Into<String>) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: Some(300),
        },
        "torrent.add_failed",
        format!("the .torrent file could not be fetched: {}", reason.into()),
    )
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
