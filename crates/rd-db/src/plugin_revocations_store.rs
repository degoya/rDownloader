//! Plugin package digests the operator withdrew.
//!
//! A signing key is revoked when its owner is no longer trusted; a *digest* is revoked when
//! one exact version of one package is. Keeping them apart is the point: withdrawing a bad
//! release must not take down every other plugin the same author signed.
//!
//! The verifier holds the set in memory and consults it on every load. This table is what
//! survives a restart: the rows are read once at start and replace that set.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// One withdrawn package version.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginDigestRevocation {
    /// `rd_plugin_host::package_digest` as 64 lowercase hex characters.
    pub digest: String,
    /// Which plugin the digest belonged to, when it was known at the time of withdrawal.
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub version: Option<String>,
    /// Why it was withdrawn, in the operator's own words.
    pub reason: Option<String>,
    pub revoked_at: String,
}

/// A withdrawal about to be recorded.
#[derive(Clone, Debug)]
pub struct NewPluginDigestRevocation {
    pub digest: String,
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub version: Option<String>,
    pub reason: Option<String>,
}

pub(crate) async fn list_plugin_digest_revocations(
    pool: &SqlitePool,
) -> Result<Vec<PluginDigestRevocation>> {
    let rows = sqlx::query_as::<_, PluginDigestRevocation>(
        "SELECT digest, plugin_id, plugin_name, version, reason, revoked_at \
         FROM plugin_digest_revocations ORDER BY revoked_at DESC, digest",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Records a withdrawal, replacing any earlier one of the same digest.
///
/// A replace rather than a refusal: withdrawing the same version twice is the operator
/// correcting the reason, not an error worth a failed request.
pub(crate) async fn insert_plugin_digest_revocation(
    connection: &mut SqliteConnection,
    input: NewPluginDigestRevocation,
) -> Result<(PluginDigestRevocation, EventEnvelope)> {
    let value = PluginDigestRevocation {
        digest: input.digest,
        plugin_id: input.plugin_id,
        plugin_name: input.plugin_name,
        version: input.version,
        reason: input.reason,
        revoked_at: Utc::now().to_rfc3339(),
    };
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO plugin_digest_revocations \
           (digest, plugin_id, plugin_name, version, reason, revoked_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(digest) DO UPDATE SET \
           plugin_id = excluded.plugin_id, \
           plugin_name = excluded.plugin_name, \
           version = excluded.version, \
           reason = excluded.reason, \
           revoked_at = excluded.revoked_at",
    )
    .bind(&value.digest)
    .bind(&value.plugin_id)
    .bind(&value.plugin_name)
    .bind(&value.version)
    .bind(&value.reason)
    .bind(&value.revoked_at)
    .execute(&mut *transaction)
    .await?;
    let event = revocation_event(&value.digest);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((value, event))
}

/// Takes a withdrawal back; returns whether one was removed.
pub(crate) async fn delete_plugin_digest_revocation(
    connection: &mut SqliteConnection,
    digest: &str,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM plugin_digest_revocations WHERE digest = ?")
        .bind(digest)
        .execute(&mut *transaction)
        .await?;
    let event = revocation_event(digest);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected() > 0, event))
}

/// What a withdrawal, or its reversal, announces.
///
/// The digest identifies the package version and is what the client refetches by; the reason
/// the operator typed stays in the table, because it is free text an administrator wrote
/// about a third party and has no business on a broadcast channel.
///
/// `PluginTrustChanged` rather than `PluginChanged`: the withdrawal routes are
/// `Secrets`-scoped, and the digest names a row in a `Secrets`-scoped table, so the event
/// follows the write.
fn revocation_event(digest: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::PluginTrustChanged,
        serde_json::json!({ "resource": "plugin_revocation", "digest": digest }),
    )
}
