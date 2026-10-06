//! The SSH host-key trust store: the verdict on a presented key, the stored keys, and
//! trusting or forgetting one.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, SshHostKey};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::changed_event;
use crate::writer::insert_event;

/// What a host key lookup found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostKeyVerdict {
    /// Exactly this key was confirmed before.
    Trusted,
    /// Nothing is stored for this endpoint and algorithm yet.
    Unknown,
    /// A different key is stored. This is either a rebuilt server or an attack, and the
    /// two are indistinguishable from here, so it is never resolved automatically.
    Changed { stored_fingerprint: String },
}

pub(crate) async fn host_key_verdict(
    pool: &SqlitePool,
    host: &str,
    port: u16,
    algorithm: &str,
    fingerprint: &str,
) -> Result<HostKeyVerdict> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT fingerprint FROM ssh_known_hosts WHERE host = ? AND port = ? AND algorithm = ?",
    )
    .bind(host)
    .bind(i64::from(port))
    .bind(algorithm)
    .fetch_optional(pool)
    .await?;
    Ok(match stored {
        None => HostKeyVerdict::Unknown,
        Some(stored) if stored == fingerprint => HostKeyVerdict::Trusted,
        Some(stored) => HostKeyVerdict::Changed {
            stored_fingerprint: stored,
        },
    })
}

pub(crate) async fn list_host_keys(pool: &SqlitePool) -> Result<Vec<SshHostKey>> {
    sqlx::query_as::<_, HostKeyRow>(
        "SELECT host, port, algorithm, fingerprint, first_seen FROM ssh_known_hosts \
         ORDER BY host, port, algorithm",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Records a host key as trusted. Replaces an existing entry, which is what confirming a
/// changed key means; the handler is responsible for making that an explicit decision.
pub(crate) async fn trust_host_key(
    connection: &mut SqliteConnection,
    key: SshHostKey,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO ssh_known_hosts (host, port, algorithm, fingerprint, first_seen) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(host, port, algorithm) DO UPDATE SET fingerprint = excluded.fingerprint",
    )
    .bind(&key.host)
    .bind(i64::from(key.port))
    .bind(&key.algorithm)
    .bind(&key.fingerprint)
    .bind(key.first_seen)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn forget_host_key(
    connection: &mut SqliteConnection,
    host: &str,
    port: u16,
    algorithm: &str,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query("DELETE FROM ssh_known_hosts WHERE host = ? AND port = ? AND algorithm = ?")
        .bind(host)
        .bind(i64::from(port))
        .bind(algorithm)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

#[derive(FromRow)]
struct HostKeyRow {
    host: String,
    port: i64,
    algorithm: String,
    fingerprint: String,
    first_seen: DateTime<Utc>,
}

impl TryFrom<HostKeyRow> for SshHostKey {
    type Error = anyhow::Error;

    fn try_from(row: HostKeyRow) -> Result<Self> {
        Ok(Self {
            host: row.host,
            port: u16::try_from(row.port).context("port out of range")?,
            algorithm: row.algorithm,
            fingerprint: row.fingerprint,
            first_seen: row.first_seen,
        })
    }
}
