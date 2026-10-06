//! Package state upkeep of the writer: settling after a download moved, deriving and writing
//! `packages.state`, and the extraction outcome.

use anyhow::Result;
use rd_core::{EventEnvelope, EventKind};
use sqlx::Connection;

use crate::writer::{Writer, insert_event};

impl Writer {
    /// The tail of every write that moves one download of a package.
    ///
    /// A row held back for the PAR2 verdict (RD-108-24) is waiting for exactly this moment:
    /// the transition that just happened may have been the last one the set was waiting for.
    /// The verdict is taken before the package state is derived, so the package settles on
    /// the states the verdict leaves behind rather than on the ones it was about to change.
    pub(crate) async fn settle_package_after_download(
        &mut self,
        package_id: rd_core::PackageId,
    ) -> Result<()> {
        let events =
            crate::nzb_queue::settle_par2_verdicts(&mut self.connection, package_id).await?;
        for event in events {
            let _ = self.events.send(event);
        }
        self.refresh_package_state(package_id).await
    }

    /// Derives `packages.state` from its files: any active file → `downloading`; files
    /// still pending → `queued`; a Usenet set given up as beyond repair → `failed`.
    /// Post-processing states are owned by the extraction service and are left alone until a
    /// file becomes active again.
    pub(crate) async fn refresh_package_state(
        &mut self,
        package_id: rd_core::PackageId,
    ) -> Result<()> {
        let Some(current) =
            sqlx::query_scalar::<_, String>("SELECT state FROM packages WHERE id = ?")
                .bind(package_id.to_string())
                .fetch_optional(&mut self.connection)
                .await?
        else {
            return Ok(());
        };
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT state, last_error_json FROM downloads WHERE package_id = ?")
                .bind(package_id.to_string())
                .fetch_all(&mut self.connection)
                .await?;
        let states: Vec<String> = rows.iter().map(|(state, _)| state.clone()).collect();
        let active = states.iter().any(|state| {
            matches!(
                state.as_str(),
                "resolving" | "downloading" | "verifying" | "repairing"
            )
        });
        // A skipped mirror never completes by design, so it must not hold the package back;
        // at least one file still has to have finished, or an all-skipped package would
        // announce itself done without a single byte.
        let all_completed = states.iter().any(|state| state == "completed")
            && states
                .iter()
                .all(|state| matches!(state.as_str(), "completed" | "skipped"));
        let next = if active {
            rd_core::PackageState::Downloading
        } else if all_completed || current == "postprocessing" {
            return Ok(());
        } else if crate::nzb_hopeless::gave_up(&rows) {
            // A Usenet set given up as beyond repair (RD-1100-02) ends here, not back in the
            // queue: nothing of it will run again unless somebody retries a row.
            rd_core::PackageState::Failed
        } else {
            rd_core::PackageState::Queued
        };
        if next.to_string() == current {
            return Ok(());
        }
        self.set_package_state(package_id, next, None, None, None)
            .await
    }

    /// Writes the package lifecycle state plus live post-processing stage and emits
    /// `package.state`.
    pub(crate) async fn set_package_state(
        &mut self,
        package_id: rd_core::PackageId,
        state: rd_core::PackageState,
        stage: Option<rd_core::PostprocessStage>,
        percent: Option<u8>,
        current: Option<String>,
    ) -> Result<()> {
        let event = EventEnvelope::new(
            EventKind::PackageState,
            serde_json::json!({
                "package_id": package_id,
                "state": state,
                "stage": stage,
                "percent": percent,
                "current": current,
            }),
        );
        let mut transaction = self.connection.begin().await?;
        // Stamped on the way into `Completed` and cleared on the way out, so a package that is
        // restarted starts its removal delay over rather than carrying the old one.
        let completed_at = (state == rd_core::PackageState::Completed).then_some(event.occurred_at);
        sqlx::query(
            "UPDATE packages SET state = ?, postprocess_stage = ?, postprocess_percent = ?, \
             postprocess_current = ?, completed_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state.to_string())
        .bind(stage.map(|value| value.to_string()))
        .bind(percent.map(i64::from))
        .bind(current)
        .bind(completed_at)
        .bind(event.occurred_at)
        .bind(package_id.to_string())
        .execute(&mut *transaction)
        .await?;
        // The history entry rides in the transaction that gives the package its outcome
        // (RD-1100-04), so it survives the package's removal and never describes an outcome
        // the queue does not know.
        let outcome = match state {
            rd_core::PackageState::Completed => Some(rd_core::HistoryOutcome::Completed),
            rd_core::PackageState::Failed => Some(rd_core::HistoryOutcome::Failed),
            _ => None,
        };
        if let Some(outcome) = outcome {
            crate::history_store::record(
                &mut transaction,
                &package_id.to_string(),
                outcome,
                event.occurred_at,
            )
            .await?;
            rd_core::failpoint!("history.before_entry_committed", || {
                anyhow::anyhow!("crash point: the history entry is written and not committed")
            });
        }
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        Ok(())
    }

    /// Persists the unpack outcome of the current post-processing run. Emits no event —
    /// the final `package.state` write follows and triggers the UI refresh.
    pub(crate) async fn set_package_extraction(
        &mut self,
        package_id: rd_core::PackageId,
        result: Option<rd_core::ExtractionResult>,
    ) -> Result<()> {
        sqlx::query("UPDATE packages SET extraction_result = ?, updated_at = ? WHERE id = ?")
            .bind(result.map(|value| value.to_string()))
            .bind(chrono::Utc::now())
            .bind(package_id.to_string())
            .execute(&mut self.connection)
            .await?;
        Ok(())
    }
}
