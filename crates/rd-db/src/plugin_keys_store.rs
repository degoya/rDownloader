//! Plugin signing keys the user confirmed on first use.
//!
//! A plugin package carries its author's public key. Installing one signed by a key the
//! user has not seen before surfaces its fingerprint for confirmation; the confirmed key
//! is recorded here so the package still verifies on the next start.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// One confirmed signing key.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginTrustedKey {
    pub key_id: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    /// Hex SHA-256 of the raw key, as shown to the user when confirming.
    pub fingerprint: String,
    /// Plugin whose installation introduced the key, for context in the UI.
    pub plugin_name: Option<String>,
    pub confirmed_at: String,
}

/// A key about to be confirmed.
#[derive(Clone, Debug)]
pub struct NewPluginTrustedKey {
    pub key_id: String,
    pub public_key: String,
    pub fingerprint: String,
    pub plugin_name: Option<String>,
}

pub(crate) async fn list_plugin_trusted_keys(pool: &SqlitePool) -> Result<Vec<PluginTrustedKey>> {
    let rows = sqlx::query_as::<_, PluginTrustedKey>(
        "SELECT key_id, public_key, fingerprint, plugin_name, confirmed_at \
         FROM plugin_trusted_keys ORDER BY confirmed_at DESC, key_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Records a confirmed key, replacing any earlier confirmation of the same key id.
pub(crate) async fn insert_plugin_trusted_key(
    connection: &mut SqliteConnection,
    input: NewPluginTrustedKey,
) -> Result<(PluginTrustedKey, EventEnvelope)> {
    let value = PluginTrustedKey {
        key_id: input.key_id,
        public_key: input.public_key,
        fingerprint: input.fingerprint,
        plugin_name: input.plugin_name,
        confirmed_at: Utc::now().to_rfc3339(),
    };
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO plugin_trusted_keys (key_id, public_key, fingerprint, plugin_name, confirmed_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(key_id) DO UPDATE SET \
           public_key = excluded.public_key, \
           fingerprint = excluded.fingerprint, \
           plugin_name = excluded.plugin_name, \
           confirmed_at = excluded.confirmed_at",
    )
    .bind(&value.key_id)
    .bind(&value.public_key)
    .bind(&value.fingerprint)
    .bind(&value.plugin_name)
    .bind(&value.confirmed_at)
    .execute(&mut *transaction)
    .await?;
    let event = key_event(&value.key_id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((value, event))
}

/// Revokes a key; returns whether a row was removed.
pub(crate) async fn delete_plugin_trusted_key(
    connection: &mut SqliteConnection,
    key_id: &str,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM plugin_trusted_keys WHERE key_id = ?")
        .bind(key_id)
        .execute(&mut *transaction)
        .await?;
    let event = key_event(key_id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected() > 0, event))
}

/// What a trust decision announces.
///
/// The key id only. The row also holds the public key and its fingerprint; neither is a
/// secret, but an event must not become a second, wider read path for a table that has a
/// narrower one.
///
/// The kind is `PluginTrustChanged`, not `PluginChanged`, for the same reason: the key
/// endpoints are `Secrets`-scoped, so the event they cause is too. As `plugin.changed` it
/// went to `Admin` subscribers instead -- the key id reached tokens that may not read the
/// table, and the `Secrets` token that made the write saw nothing.
fn key_event(key_id: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::PluginTrustChanged,
        serde_json::json!({ "resource": "plugin_key", "key_id": key_id }),
    )
}
