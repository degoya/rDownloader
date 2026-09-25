//! Persistence of reusable per-domain session and authentication profiles.
//!
//! Credential values never reach this module; it only stores the opaque `vault://`
//! references minted by the secret store.

use std::cmp::Ordering;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    AuthMethod, AuthOrigin, AuthProfile, AuthProfileId, AuthScope, EventEnvelope, EventKind,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{
    error::StoreError,
    network_store::{enum_string, parse_enum},
    writer::insert_event,
};

/// Editable profile fields; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewAuthProfile {
    pub name: String,
    pub scope: AuthScope,
    pub method: AuthMethod,
    pub origin: AuthOrigin,
    pub enabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    pub certificate_ref: Option<String>,
}

/// Fields a profile update may change. Secret references are replaced wholesale; the
/// caller cleans up the ones that fall out of use.
#[derive(Clone, Debug)]
pub struct UpdateAuthProfile {
    pub name: String,
    pub scope: AuthScope,
    pub method: AuthMethod,
    pub enabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    pub certificate_ref: Option<String>,
}

const COLUMNS: &str = "id, name, host, include_subdomains, path_prefix, method, origin, enabled, \
     expires_at, username, secret_ref, certificate_ref, created_at, updated_at";

/// Points one job at a profile, at no profile, or back at scope matching.
pub(crate) async fn set_download_selection(
    connection: &mut SqliteConnection,
    id: rd_core::DownloadId,
    selection: rd_core::AuthProfileSelection,
) -> Result<()> {
    let (profile_id, pinned) = selection.to_columns();
    let updated = sqlx::query(
        "UPDATE downloads SET auth_profile_id = ?, auth_profile_pinned = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(profile_id.map(|id| id.to_string()))
    .bind(pinned)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("download not found"));
    }
    Ok(())
}

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::AuthProfileChanged,
        serde_json::json!({ "resource": "auth_profile" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<AuthProfile>> {
    sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles ORDER BY host, name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(pool: &SqlitePool, id: AuthProfileId) -> Result<Option<AuthProfile>> {
    sqlx::query_as::<_, ProfileRow>(&format!("SELECT {COLUMNS} FROM auth_profiles WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

/// Returns the most specific enabled, unexpired profile covering `url`.
///
/// Expired profiles are skipped silently here: auto-matching is a convenience, so a stale
/// session must not turn every download of that host into a hard failure. A job that pins
/// a profile explicitly gets the opposite treatment (see `client_config`).
pub(crate) async fn match_for_url(pool: &SqlitePool, url: &Url) -> Result<Option<AuthProfile>> {
    if url.host_str().is_none() {
        return Ok(None);
    }
    let now = Utc::now();
    // Candidate set is narrowed by SQL to the host and its parent domains; the precise
    // subdomain and path rules live in AuthScope so they stay testable in one place.
    let candidates = sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles \
         WHERE enabled = 1 AND (expires_at IS NULL OR expires_at > ?)"
    ))
    .bind(now)
    .fetch_all(pool)
    .await?;
    let mut best: Option<AuthProfile> = None;
    for row in candidates {
        let profile: AuthProfile = row.try_into()?;
        if !profile.scope.matches_url(url) {
            continue;
        }
        let better = best.as_ref().is_none_or(|current| {
            match profile
                .scope
                .specificity()
                .cmp(&current.scope.specificity())
            {
                Ordering::Greater => true,
                // UUIDv7 is time ordered, so the older profile wins a tie deterministically.
                Ordering::Equal => profile.id < current.id,
                Ordering::Less => false,
            }
        });
        if better {
            best = Some(profile);
        }
    }
    Ok(best)
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewAuthProfile,
) -> Result<(AuthProfile, EventEnvelope)> {
    let now = Utc::now();
    let value = AuthProfile {
        id: AuthProfileId::new(),
        name: input.name,
        scope: input.scope,
        method: input.method,
        origin: input.origin,
        // A capture client hands over a live browser session; it must never arrive ready
        // to use. Enforced here rather than in the handler so no future caller can skip it.
        enabled: input.enabled && input.origin != AuthOrigin::BrowserCapture,
        expires_at: input.expires_at,
        username: input.username,
        has_secret: input.secret_ref.is_some(),
        has_client_certificate: input.certificate_ref.is_some(),
        secret_ref: input.secret_ref,
        certificate_ref: input.certificate_ref,
        created_at: now,
        updated_at: now,
    };
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO auth_profiles (id, name, host, include_subdomains, path_prefix, method, \
         origin, enabled, expires_at, username, secret_ref, certificate_ref, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(&value.scope.host)
    .bind(value.scope.include_subdomains)
    .bind(&value.scope.path_prefix)
    .bind(enum_string(value.method)?)
    .bind(enum_string(value.origin)?)
    .bind(value.enabled)
    .bind(value.expires_at)
    .bind(&value.username)
    .bind(&value.secret_ref)
    .bind(&value.certificate_ref)
    .bind(value.created_at)
    .bind(value.updated_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        crate::error::tag_duplicate(error, "another auth profile already covers this scope")
    })?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Applies an update and returns the profile plus the references it no longer uses.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: AuthProfileId,
    input: UpdateAuthProfile,
) -> Result<(AuthProfile, Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let previous = sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("auth profile not found"))?;
    sqlx::query(
        "UPDATE auth_profiles SET name = ?, host = ?, include_subdomains = ?, path_prefix = ?, \
         method = ?, enabled = ?, expires_at = ?, username = ?, secret_ref = ?, \
         certificate_ref = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(&input.scope.host)
    .bind(input.scope.include_subdomains)
    .bind(&input.scope.path_prefix)
    .bind(enum_string(input.method)?)
    .bind(input.enabled)
    .bind(input.expires_at)
    .bind(&input.username)
    .bind(&input.secret_ref)
    .bind(&input.certificate_ref)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        crate::error::tag_duplicate(error, "another auth profile already covers this scope")
    })?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [
        (previous.secret_ref, &input.secret_ref),
        (previous.certificate_ref, &input.certificate_ref),
    ]
    .into_iter()
    .filter_map(|(old, new)| old.filter(|old| Some(old) != new.as_ref()))
    .collect();
    let value = sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((value, orphaned, event))
}

/// Enables or disables a profile; this is how a captured browser session is approved.
pub(crate) async fn set_enabled(
    connection: &mut SqliteConnection,
    id: AuthProfileId,
    enabled: bool,
) -> Result<(AuthProfile, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let updated = sqlx::query("UPDATE auth_profiles SET enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("auth profile not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let value = sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((value, event))
}

/// Deletes a profile and returns the secret references left behind.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: AuthProfileId,
) -> Result<(Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let existing = sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {COLUMNS} FROM auth_profiles WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("auth profile not found"))?;
    // Pinned jobs fall back to auto-matching rather than silently downloading with a
    // credential the user just removed.
    sqlx::query("UPDATE downloads SET auth_profile_id = NULL WHERE auth_profile_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM auth_profiles WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [existing.secret_ref, existing.certificate_ref]
        .into_iter()
        .flatten()
        .collect();
    Ok((orphaned, event))
}

#[derive(FromRow)]
struct ProfileRow {
    id: String,
    name: String,
    host: String,
    include_subdomains: bool,
    path_prefix: Option<String>,
    method: String,
    origin: String,
    enabled: bool,
    expires_at: Option<DateTime<Utc>>,
    username: Option<String>,
    secret_ref: Option<String>,
    certificate_ref: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ProfileRow> for AuthProfile {
    type Error = anyhow::Error;

    fn try_from(row: ProfileRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            name: row.name,
            scope: AuthScope {
                host: row.host,
                include_subdomains: row.include_subdomains,
                path_prefix: row.path_prefix,
            },
            method: parse_enum(&row.method)?,
            origin: parse_enum(&row.origin)?,
            enabled: row.enabled,
            expires_at: row.expires_at,
            username: row.username,
            has_secret: row.secret_ref.is_some(),
            has_client_certificate: row.certificate_ref.is_some(),
            secret_ref: row.secret_ref,
            certificate_ref: row.certificate_ref,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}
