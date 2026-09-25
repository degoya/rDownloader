//! Persistence of stored FTP/FTPS/SFTP logins and the SSH host-key trust store.
//!
//! Credential values never reach this module; it only stores the opaque `vault://`
//! references minted by the secret store, exactly like [`crate::auth_profile_store`].

use std::cmp::Ordering;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    EventEnvelope, EventKind, RemoteAuthMode, RemoteCredential, RemoteCredentialId, RemoteProtocol,
    RemoteTarget, SshHostKey,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{
    error::StoreError,
    network_store::{enum_string, parse_enum},
    writer::insert_event,
};

/// Editable credential fields; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewRemoteCredential {
    pub name: String,
    pub protocol: RemoteProtocol,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub auth_mode: RemoteAuthMode,
    pub passive: bool,
    pub enabled: bool,
    pub secret_ref: Option<String>,
    pub key_ref: Option<String>,
    pub passphrase_ref: Option<String>,
}

/// Fields a credential update may change. Secret references are replaced wholesale; the
/// caller cleans up the ones that fall out of use.
#[derive(Clone, Debug)]
pub struct UpdateRemoteCredential {
    pub name: String,
    pub protocol: RemoteProtocol,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub auth_mode: RemoteAuthMode,
    pub passive: bool,
    pub enabled: bool,
    pub secret_ref: Option<String>,
    pub key_ref: Option<String>,
    pub passphrase_ref: Option<String>,
}

const COLUMNS: &str = "id, name, protocol, host, port, username, auth_mode, passive, secret_ref, \
     key_ref, passphrase_ref, enabled, created_at, updated_at";

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::RemoteCredentialChanged,
        serde_json::json!({ "resource": "remote_credential" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<RemoteCredential>> {
    sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials ORDER BY host, port, name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(
    pool: &SqlitePool,
    id: RemoteCredentialId,
) -> Result<Option<RemoteCredential>> {
    sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// Returns the most specific enabled credential that can serve `target`.
///
/// A credential naming the same user beats a catch-all for the endpoint; ties go to the
/// older id, so the choice is deterministic across restarts. Selection rules live in
/// [`RemoteCredential::matches`] so they stay testable without a database.
pub(crate) async fn match_for_target(
    pool: &SqlitePool,
    target: &RemoteTarget,
) -> Result<Option<RemoteCredential>> {
    // SQL narrows by endpoint; the family and user rules stay in one place in rd-core.
    let candidates = sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials WHERE enabled = 1 AND host = ? AND port = ?"
    ))
    .bind(&target.host)
    .bind(i64::from(target.port))
    .fetch_all(pool)
    .await?;
    let mut best: Option<RemoteCredential> = None;
    for row in candidates {
        let credential: RemoteCredential = row.try_into()?;
        if !credential.matches(target) {
            continue;
        }
        let better = best.as_ref().is_none_or(|current| {
            match credential.specificity().cmp(&current.specificity()) {
                Ordering::Greater => true,
                Ordering::Equal => credential.id < current.id,
                Ordering::Less => false,
            }
        });
        if better {
            best = Some(credential);
        }
    }
    Ok(best)
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewRemoteCredential,
) -> Result<(RemoteCredential, EventEnvelope)> {
    let now = Utc::now();
    let value = RemoteCredential {
        id: RemoteCredentialId::new(),
        name: input.name,
        protocol: input.protocol,
        host: input.host,
        port: input.port,
        username: input.username,
        auth_mode: input.auth_mode,
        passive: input.passive,
        enabled: input.enabled,
        has_secret: input.secret_ref.is_some(),
        has_key: input.key_ref.is_some(),
        has_passphrase: input.passphrase_ref.is_some(),
        secret_ref: input.secret_ref,
        key_ref: input.key_ref,
        passphrase_ref: input.passphrase_ref,
        created_at: now,
        updated_at: now,
    };
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO remote_credentials (id, name, protocol, host, port, username, auth_mode, \
         passive, secret_ref, key_ref, passphrase_ref, enabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(enum_string(value.protocol)?)
    .bind(&value.host)
    .bind(i64::from(value.port))
    .bind(&value.username)
    .bind(enum_string(value.auth_mode)?)
    .bind(value.passive)
    .bind(&value.secret_ref)
    .bind(&value.key_ref)
    .bind(&value.passphrase_ref)
    .bind(value.enabled)
    .bind(value.created_at)
    .bind(value.updated_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        crate::error::tag_duplicate(
            error,
            "a remote login for this server, port and user already exists",
        )
    })?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Applies an update and returns the credential plus the references it no longer uses.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: RemoteCredentialId,
    input: UpdateRemoteCredential,
) -> Result<(RemoteCredential, Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let previous = sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("remote credential not found"))?;
    sqlx::query(
        "UPDATE remote_credentials SET name = ?, protocol = ?, host = ?, port = ?, username = ?, \
         auth_mode = ?, passive = ?, secret_ref = ?, key_ref = ?, passphrase_ref = ?, \
         enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(enum_string(input.protocol)?)
    .bind(&input.host)
    .bind(i64::from(input.port))
    .bind(&input.username)
    .bind(enum_string(input.auth_mode)?)
    .bind(input.passive)
    .bind(&input.secret_ref)
    .bind(&input.key_ref)
    .bind(&input.passphrase_ref)
    .bind(input.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        crate::error::tag_duplicate(
            error,
            "a remote login for this server, port and user already exists",
        )
    })?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [
        (previous.secret_ref, &input.secret_ref),
        (previous.key_ref, &input.key_ref),
        (previous.passphrase_ref, &input.passphrase_ref),
    ]
    .into_iter()
    .filter_map(|(old, new)| old.filter(|old| Some(old) != new.as_ref()))
    .collect();
    let value = sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((value, orphaned, event))
}

/// Deletes a credential and returns the secret references left behind.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: RemoteCredentialId,
) -> Result<(Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let existing = sqlx::query_as::<_, CredentialRow>(&format!(
        "SELECT {COLUMNS} FROM remote_credentials WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("remote credential not found"))?;
    // Jobs that named this login fall back to matching by endpoint rather than silently
    // transferring with a credential the user just removed.
    sqlx::query("UPDATE downloads SET remote_credential_id = NULL WHERE remote_credential_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM remote_credentials WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [
        existing.secret_ref,
        existing.key_ref,
        existing.passphrase_ref,
    ]
    .into_iter()
    .flatten()
    .collect();
    Ok((orphaned, event))
}

// --- Candidate listings ---------------------------------------------------------------

/// Reads the reviewed listing of one link candidate.
pub(crate) async fn candidate_listing(
    pool: &SqlitePool,
    id: rd_core::CandidateId,
) -> Result<Option<rd_core::RemoteCandidateState>> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT listing_json FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    stored
        .flatten()
        .map(|value| serde_json::from_str(&value).context("parse candidate remote listing"))
        .transpose()
}

/// Stores a freshly probed listing together with the login that produced it, keeping any
/// selection the user already made for the same root.
pub(crate) async fn set_candidate_listing(
    connection: &mut SqliteConnection,
    id: rd_core::CandidateId,
    listing: rd_core::RemoteListing,
    credential_id: Option<RemoteCredentialId>,
) -> Result<()> {
    let mut tx = connection.begin().await?;
    let previous: Option<String> =
        sqlx::query_scalar("SELECT listing_json FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
    let mut state = rd_core::RemoteCandidateState::resolved(listing);
    // A re-check of the same directory must not throw away what the user already
    // deselected; a listing of a different root is a different review.
    if let Some(previous) = previous
        .as_deref()
        .and_then(|value| serde_json::from_str::<rd_core::RemoteCandidateState>(value).ok())
        && previous.listing.root == state.listing.root
    {
        state.plan = previous.plan;
    }
    sqlx::query(
        "UPDATE link_candidates SET listing_json = ?, remote_credential_id = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&state)?)
    .bind(credential_id.map(|id| id.to_string()))
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Replaces the selection of one candidate and returns the resolved listing.
pub(crate) async fn set_candidate_listing_plan(
    connection: &mut SqliteConnection,
    id: rd_core::CandidateId,
    plan: rd_core::RemoteListingPlan,
) -> Result<rd_core::ResolvedRemoteListing> {
    let mut tx = connection.begin().await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT listing_json FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
    let mut state: rd_core::RemoteCandidateState = stored
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("parse candidate remote listing")?
        .context("candidate has no remote listing")?;
    state.plan = plan;
    let resolved = state.resolve();
    sqlx::query("UPDATE link_candidates SET listing_json = ? WHERE id = ?")
        .bind(serde_json::to_string(&state)?)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(resolved)
}

// --- SSH host key trust store -------------------------------------------------------

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
struct CredentialRow {
    id: String,
    name: String,
    protocol: String,
    host: String,
    port: i64,
    username: Option<String>,
    auth_mode: String,
    passive: bool,
    secret_ref: Option<String>,
    key_ref: Option<String>,
    passphrase_ref: Option<String>,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<CredentialRow> for RemoteCredential {
    type Error = anyhow::Error;

    fn try_from(row: CredentialRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            name: row.name,
            protocol: parse_enum(&row.protocol)?,
            host: row.host,
            port: u16::try_from(row.port).context("port out of range")?,
            username: row.username,
            auth_mode: parse_enum(&row.auth_mode)?,
            passive: row.passive,
            enabled: row.enabled,
            has_secret: row.secret_ref.is_some(),
            has_key: row.key_ref.is_some(),
            has_passphrase: row.passphrase_ref.is_some(),
            secret_ref: row.secret_ref,
            key_ref: row.key_ref,
            passphrase_ref: row.passphrase_ref,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
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
