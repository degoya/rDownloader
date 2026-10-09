//! Resolver pins that are set on purpose or can no longer be honoured (RD-140-02).
//!
//! A pin keeps a download on one exact resolver version. `claim_resolver_pin` writes the first
//! one when a job starts; this module holds the two other ways a pin changes: the operator
//! points a download at the version under test, and a start finds a pin whose version is gone.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{DownloadId, EventEnvelope, EventKind, Failure, FailureKind};
use sqlx::{Connection, Row};

use crate::{
    error::StoreError,
    writer::{Writer, insert_event},
};

/// The code a download carries once the version it was pinned to has been withdrawn.
pub(crate) const PINNED_VERSION_WITHDRAWN: &str = "plugin.pinned_version_withdrawn";

/// States a download would leave on its own at the next start. A download pinned to a
/// withdrawn version is held in `blocked` instead, so it runs again only when a person says so.
const RUNS_ON_ITS_OWN: [&str; 4] = ["queued", "retry_wait", "resolving", "downloading"];

/// States that already wait for a person; they keep their state and carry the reason.
const WAITS_FOR_A_PERSON: [&str; 4] = ["paused", "failed", "blocked", "cancelled"];

impl Writer {
    /// Replaces a download's resolver pin; refused while the download is running.
    pub(crate) async fn pin_download_resolver(
        &mut self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<()> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if current.state.holds_the_file() {
            bail!(StoreError::wrong_state(
                "a running download keeps the version it started with; pause it first"
            ));
        }
        sqlx::query(
            "INSERT INTO download_resolver_pins \
             (download_id, plugin_id, plugin_version, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(download_id) DO UPDATE SET \
               plugin_id = excluded.plugin_id, \
               plugin_version = excluded.plugin_version, \
               created_at = excluded.created_at",
        )
        .bind(id.to_string())
        .bind(pin.plugin_id.to_string())
        .bind(&pin.version)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }

    /// Drops a download's resolver pin so its next start resolves with the plugin installed
    /// now (RD-1210-01); answers the pin it had. Refused while the download holds its file, for
    /// the reason [`Self::pin_download_resolver`] is: a running job keeps its version.
    ///
    /// The transfer's chunks, ETag and size stay as they are. Whether the bytes already on disk
    /// may be kept is decided where it always is, when the next attempt plans its transfer: the
    /// new resolution's size and validators against the recorded ones.
    pub(crate) async fn release_resolver_pin(
        &mut self,
        id: DownloadId,
    ) -> Result<Option<rd_core::ResolverPin>> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if current.state.holds_the_file() {
            bail!(StoreError::wrong_state(
                "a running download keeps the version it started with; pause it first"
            ));
        }
        let previous = sqlx::query(
            "DELETE FROM download_resolver_pins WHERE download_id = ? \
             RETURNING plugin_id, plugin_version",
        )
        .bind(id.to_string())
        .fetch_optional(&mut self.connection)
        .await?;
        previous
            .map(|row| {
                Ok(rd_core::ResolverPin {
                    plugin_id: crate::parse_id(row.get::<String, _>("plugin_id").as_str())?,
                    version: row.get("plugin_version"),
                })
            })
            .transpose()
    }

    /// Drops pins naming a resolver version this build can no longer provide.
    ///
    /// A pin keeps a *running* job on one exact resolver version, which is what makes a
    /// mid-download plugin upgrade safe. It was never meant to outlive the version it names:
    /// once that build is gone — an ABI break, a removed third-party plugin — the pin can
    /// never be satisfied again and the job dies with `plugin.pinned_version_missing` on
    /// every retry, forever. The plugin id is stable across versions, so clearing the pin
    /// lets the job resolve through the current build of the same plugin, which is the
    /// outcome the pin was protecting in the first place.
    ///
    /// A version that was **withdrawn** is different (RD-140-02): somebody decided that build
    /// must not run, and moving its jobs to another build without a word is exactly the
    /// silence the withdrawal should not meet. Its pins are released too — the job could never
    /// run on that build again — but a download that would have started on its own is held in
    /// `blocked`, and every one that still has work to do carries
    /// [`PINNED_VERSION_WITHDRAWN`] with the plugin and the version, so the queue says why.
    /// Resuming it runs it on the plugin's current default version. A version counts as
    /// withdrawn when a withdrawal names its plugin id and version; one recorded by digest
    /// alone cannot be matched to a pin and is released like a missing version.
    ///
    /// Runs at startup, before the scheduler starts anything, so no running job loses its pin.
    pub(crate) async fn clear_unsatisfiable_resolver_pins(
        &mut self,
        available: &[(String, String)],
    ) -> Result<u64> {
        let rows = sqlx::query(
            "SELECT pin.download_id, pin.plugin_id, pin.plugin_version, download.state, \
               EXISTS (SELECT 1 FROM plugin_digest_revocations revocation \
                       WHERE revocation.plugin_id = pin.plugin_id \
                         AND revocation.version = pin.plugin_version) AS withdrawn \
             FROM download_resolver_pins pin \
             JOIN downloads download ON download.id = pin.download_id",
        )
        .fetch_all(&mut self.connection)
        .await?;
        let mut stale = Vec::new();
        for row in rows {
            let plugin_id: String = row.get("plugin_id");
            let version: String = row.get("plugin_version");
            if available
                .iter()
                .any(|(id, installed)| id == &plugin_id && installed == &version)
            {
                continue;
            }
            stale.push((
                row.get::<String, _>("download_id"),
                plugin_id,
                version,
                row.get::<String, _>("state"),
                row.get::<bool, _>("withdrawn"),
            ));
        }
        let mut transaction = self.connection.begin().await?;
        let mut events = Vec::new();
        for (download_id, plugin_id, version, state, withdrawn) in &stale {
            sqlx::query("DELETE FROM download_resolver_pins WHERE download_id = ?")
                .bind(download_id)
                .execute(&mut *transaction)
                .await?;
            if !*withdrawn {
                continue;
            }
            let next = if RUNS_ON_ITS_OWN.contains(&state.as_str()) {
                "blocked"
            } else if WAITS_FOR_A_PERSON.contains(&state.as_str()) {
                state.as_str()
            } else {
                // Finished, or past the resolver: nothing left for the version to do.
                continue;
            };
            let failure = withdrawn_failure(plugin_id, version);
            let event = EventEnvelope::new(
                EventKind::DownloadState,
                serde_json::json!({
                    "download_id": download_id,
                    "state": next,
                    "failure": failure,
                }),
            );
            sqlx::query(
                "UPDATE downloads SET state = ?, next_retry_at = NULL, last_error_json = ?, \
                 updated_at = ? WHERE id = ?",
            )
            .bind(next)
            .bind(serde_json::to_string(&failure)?)
            .bind(event.occurred_at)
            .bind(download_id)
            .execute(&mut *transaction)
            .await?;
            insert_event(&mut transaction, &event).await?;
            events.push(event);
        }
        transaction.commit().await?;
        for event in events {
            let _ = self.events.send(event);
        }
        Ok(stale.len() as u64)
    }
}

/// What a download pinned to a withdrawn version reports, translated by its code.
fn withdrawn_failure(plugin_id: &str, version: &str) -> Failure {
    Failure::coded(
        FailureKind::Unsupported,
        PINNED_VERSION_WITHDRAWN,
        "The plugin version this download was pinned to has been withdrawn; resume it to \
         continue with the plugin's current version",
    )
    .with_param("plugin_id", plugin_id)
    .with_param("version", version)
}

#[cfg(test)]
#[path = "writer_pins_tests.rs"]
mod tests;
