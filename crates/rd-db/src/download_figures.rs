//! The queue's figures: state and bytes of every row, without the rows (RD-1480-19).
//!
//! `Database::list_downloads` joins the packages, orders the result and decodes every column,
//! JSON blobs included. The rates, the queue summary, the tray's figures and the power
//! supervisor need four columns of it, and three of the four ask every few seconds at most.

use anyhow::{Context, Result};
use rd_core::DownloadFigures;

use crate::{Database, models::i64_to_bytes, parse_id};

impl Database {
    /// State and bytes of every download, in no particular order.
    pub async fn download_figures(&self) -> Result<Vec<DownloadFigures>> {
        sqlx::query_as::<_, (String, String, Option<i64>, i64)>(
            "SELECT id, state, total_bytes, committed_bytes FROM downloads",
        )
        .fetch_all(&self.readers)
        .await?
        .into_iter()
        .map(|(id, state, total_bytes, committed_bytes)| {
            Ok(DownloadFigures {
                id: parse_id(&id)?,
                state: state.parse().context("parse download state")?,
                total_bytes: total_bytes.map(i64_to_bytes).transpose()?,
                committed_bytes: i64_to_bytes(committed_bytes)?,
            })
        })
        .collect()
    }
}
