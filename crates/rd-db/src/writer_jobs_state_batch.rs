//! Pausing and resuming many rows in one transaction (RD-1250-08).
//!
//! "Pause all" over 1145 files used to be 1145 requests, and every one a transaction and an
//! event of its own. A batch is one transaction and one `download.state` event, as the removal
//! of many rows is (RD-1120-17).

use anyhow::Result;
use rd_core::{DownloadId, DownloadState};
use sqlx::{Connection, Row};

use crate::{
    error::StoreError,
    writer::{Writer, insert_event, rows_event},
};

/// What a batch does to a row.
#[derive(Clone, Copy)]
enum Move {
    /// A waiting row to `paused`; any other row stays as it is, as the single pause leaves it.
    Pause,
    /// Back to `queued`, a failed row with a fresh retry budget as the single resume gives it;
    /// refused where the state does not allow it.
    Resume,
}

impl Move {
    fn next(self) -> DownloadState {
        match self {
            Self::Pause => DownloadState::Paused,
            Self::Resume => DownloadState::Queued,
        }
    }
}

impl Writer {
    /// Pauses every row of `ids` that waits — `queued` or `retry_wait` — in one transaction.
    ///
    /// One answer per id, in order: `true` for a row that moved, `false` for one in any other
    /// state, left as it was without a refusal, and a refusal for a row that is gone. A row a
    /// worker runs is the scheduler's to stop; the writer only sees its state.
    pub(crate) async fn pause_downloads(
        &mut self,
        ids: &[DownloadId],
    ) -> Result<Vec<Result<bool>>> {
        self.move_downloads(ids, Move::Pause).await
    }

    /// Queues every row of `ids` again in one transaction, as the single resume does: a failed
    /// row with a fresh retry budget, any other with the ordinary transition, refused on its
    /// own where that is not allowed.
    ///
    /// A waiting mirror — `skipped` in a mirror group — is answered `false` and left as it is:
    /// its resume is a choice between links, which stands its siblings down first, and that is
    /// the scheduler's single resume to make.
    pub(crate) async fn resume_downloads(
        &mut self,
        ids: &[DownloadId],
    ) -> Result<Vec<Result<bool>>> {
        self.move_downloads(ids, Move::Resume).await
    }

    /// The batch both directions share: every row checked and written in one transaction, the
    /// rows that moved announced with one event ([`rows_event`], with their new `state` and no
    /// `previous`: a pause or a resume is no trigger for automations or notifications), and
    /// each package settled once afterwards, as a single transition settles its package.
    async fn move_downloads(
        &mut self,
        ids: &[DownloadId],
        change: Move,
    ) -> Result<Vec<Result<bool>>> {
        let next = change.next();
        let now = chrono::Utc::now();
        let mut outcomes = Vec::with_capacity(ids.len());
        let mut moved = Vec::new();
        let mut announced = std::collections::HashSet::new();
        let mut packages = Vec::new();
        let mut transaction = self.connection.begin().await?;
        for &id in ids {
            let Some(row) = sqlx::query(
                "SELECT state, package_id, mirror_group IS NOT NULL AS mirrored \
                 FROM downloads WHERE id = ?",
            )
            .bind(id.to_string())
            .fetch_optional(&mut *transaction)
            .await?
            else {
                outcomes.push(Err(anyhow::anyhow!(StoreError::not_found(
                    "download not found"
                ))));
                continue;
            };
            let current: DownloadState = row.get::<String, _>("state").parse()?;
            let package_id: String = row.get("package_id");
            match change {
                Move::Pause
                    if !matches!(current, DownloadState::Queued | DownloadState::RetryWait) =>
                {
                    outcomes.push(Ok(false));
                    continue;
                }
                Move::Resume
                    if current == DownloadState::Skipped && row.get::<i64, _>("mirrored") != 0 =>
                {
                    outcomes.push(Ok(false));
                    continue;
                }
                Move::Resume if current == DownloadState::Failed => {
                    sqlx::query(
                        "UPDATE downloads SET state = 'queued', retry_count = 0, limit_waits = 0, \
                         next_retry_at = NULL, last_error_json = NULL, block_reason = NULL, \
                         updated_at = ? WHERE id = ?",
                    )
                    .bind(now)
                    .bind(id.to_string())
                    .execute(&mut *transaction)
                    .await?;
                }
                _ if !current.can_transition_to(next) => {
                    outcomes.push(Err(anyhow::anyhow!(StoreError::wrong_state(format!(
                        "invalid download transition {current} -> {next}"
                    )))));
                    continue;
                }
                _ => {
                    sqlx::query(
                        "UPDATE downloads SET state = ?, \
                         last_error_json = CASE WHEN ? = 'queued' THEN NULL ELSE last_error_json END, \
                         block_reason = NULL, updated_at = ? WHERE id = ?",
                    )
                    .bind(next.to_string())
                    .bind(next.to_string())
                    .bind(now)
                    .bind(id.to_string())
                    .execute(&mut *transaction)
                    .await?;
                }
            }
            // A second mention of an id finds it moved already: a paused row stays as it is,
            // and a queued one is queued again, as the single resume allows a transition to the
            // state a row is in. The event names it once.
            if announced.insert(id) {
                moved.push(id);
            }
            if !packages.contains(&package_id) {
                packages.push(package_id);
            }
            outcomes.push(Ok(true));
        }
        let event =
            (!moved.is_empty()).then(|| rows_event(&moved, serde_json::json!({ "state": next })));
        if let Some(event) = &event {
            insert_event(&mut transaction, event).await?;
        }
        transaction.commit().await?;
        if let Some(event) = event {
            let _ = self.events.send(event);
        }
        // Committed whatever this answers, as after a removal of many rows.
        for package_id in packages {
            let Ok(package_id) = package_id.parse::<rd_core::PackageId>() else {
                continue;
            };
            if let Err(error) = self.settle_package_after_download(package_id).await {
                tracing::warn!(%package_id, %error, "the package was not settled after a batch");
            }
        }
        Ok(outcomes)
    }
}
