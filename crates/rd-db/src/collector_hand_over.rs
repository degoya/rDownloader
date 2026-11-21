//! LinkGrabber links handed to a remote-job provider (RD-1440-07): taken out of the LinkGrabber,
//! or left in it marked with the job they went to, in one transaction.
//!
//! The job rows are written before this, by the remote-job service's duplicate guard. A crash in
//! between leaves a job whose link is neither removed nor marked; the next hand-over of the
//! package meets that guard (`AlreadyOurs`), counts the link as handed over and settles it then,
//! the way an NZB import's mark is healed (RD-191-13).

use anyhow::Result;
use rd_core::{CandidateId, EventEnvelope, EventKind, RemoteJobId};
use sqlx::{Connection, SqliteConnection};

use crate::{Database, collector_store, commands::CollectorCommand, writer};

/// What becomes of the links that reached a provider; the setting decides, never the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandedOverLinks {
    /// They leave the LinkGrabber; a package left without links goes with them.
    Remove,
    /// They stay, each naming its job, so a second hand-over sends nothing again.
    Mark,
}

/// Removes or marks `handed` and answers how many rows changed.
///
/// A link being queued (`resolving`) or already queued (`enqueued`) is left alone: the enqueue
/// owns it, and its row is not the LinkGrabber's to remove any more.
pub(crate) async fn settle(
    connection: &mut SqliteConnection,
    handed: &[(CandidateId, RemoteJobId)],
    links: HandedOverLinks,
) -> Result<(u64, EventEnvelope)> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({
            "handed_over": handed.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            "removed": links == HandedOverLinks::Remove,
        }),
    );
    let mut transaction = connection.begin().await?;
    let mut changed = 0u64;
    for (candidate, job) in handed {
        let result = match links {
            HandedOverLinks::Remove => {
                sqlx::query(
                    "DELETE FROM link_candidates WHERE id = ? \
                     AND state NOT IN ('resolving', 'enqueued')",
                )
                .bind(candidate.to_string())
                .execute(&mut *transaction)
                .await?
            }
            HandedOverLinks::Mark => {
                sqlx::query(
                    "UPDATE link_candidates SET remote_job_id = ? WHERE id = ? \
                     AND state NOT IN ('resolving', 'enqueued')",
                )
                .bind(job.to_string())
                .bind(candidate.to_string())
                .execute(&mut *transaction)
                .await?
            }
        };
        changed += result.rows_affected();
    }
    if links == HandedOverLinks::Remove {
        collector_store::delete_empty_batches(&mut transaction).await?;
    }
    collector_store::insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((changed, event))
}

impl Database {
    /// Settles the LinkGrabber links a hand-over sent to a provider (RD-1440-07): removes them,
    /// with a package they leave empty and the secrets their rows held, or marks each with its
    /// job. Answers how many links changed.
    pub async fn settle_handed_over_candidates(
        &self,
        handed: Vec<(CandidateId, RemoteJobId)>,
        links: HandedOverLinks,
    ) -> Result<u64> {
        if handed.is_empty() {
            return Ok(0);
        }
        // Read before the delete: the references are columns of the rows being removed.
        let mut orphaned = Vec::new();
        if links == HandedOverLinks::Remove {
            for (id, _) in &handed {
                orphaned.extend(
                    collector_store::candidate_vault_refs(&self.readers, *id)
                        .await
                        .unwrap_or_default(),
                );
            }
        }
        let changed = writer::request(&self.writer, |reply| {
            CollectorCommand::SettleHandedOverCandidates {
                handed,
                links,
                reply,
            }
        })
        .await?;
        if links == HandedOverLinks::Remove {
            self.forget_secrets(orphaned).await;
            // A package the links leave empty goes, and its archive password (RD-190-04).
            self.sweep_archive_passwords().await;
        }
        Ok(changed)
    }
}
