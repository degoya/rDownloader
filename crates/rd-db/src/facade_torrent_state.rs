//! Database facade for editing a queue row's torrent state in place.
//!
//! The state is one JSON blob that several writers change: the runner, the seeding
//! supervisor, a recheck, a scrape and the REST controls. Each of them reading the blob,
//! changing a field and writing the whole blob back would drop whatever another writer stored
//! in between. An edit sent here runs inside the single writer task instead, between its own
//! read and write, so two edits of different fields both survive.

use anyhow::{Context, Result};
use rd_core::{DownloadId, TorrentJobState};

use crate::{
    Database,
    commands::{DownloadsCommand, TorrentStateEdit},
    writer,
};

impl Database {
    /// Applies `edit` to the stored torrent state of one queue row and stores the result.
    ///
    /// A row without state starts from the defaults. An `Err` from `edit` leaves the row
    /// untouched and is handed back as the inner error; `Ok` carries the state as stored and
    /// the value `edit` returned. The outer error is the database's: a missing row, an
    /// unreadable blob or a failed write.
    ///
    /// `edit` runs on the writer task, so it must be quick and must not block.
    pub async fn update_download_torrent_state<T, E>(
        &self,
        id: DownloadId,
        edit: impl FnOnce(&mut TorrentJobState) -> Result<T, E> + Send + 'static,
    ) -> Result<Result<(TorrentJobState, T), E>>
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        let (outcome_sender, outcome) = tokio::sync::oneshot::channel();
        let edit = TorrentStateEdit::new(move |state: &mut TorrentJobState| {
            let result = edit(state);
            let write = result.is_ok();
            let _ = outcome_sender.send(result);
            write
        });
        let stored = writer::request(&self.writer, |reply| {
            DownloadsCommand::UpdateDownloadTorrentState { id, edit, reply }
        })
        .await?;
        let outcome = outcome.await.context("torrent state edit did not run")?;
        Ok(outcome.map(|value| (stored, value)))
    }
}
