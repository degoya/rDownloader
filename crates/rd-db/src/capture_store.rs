use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{CaptureToken, CaptureTokenId, EventEnvelope, EventKind};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

/// Issues a token and records that it was issued, in one transaction.
///
/// The event is not decoration. Until scopes could be changed after the fact, the only
/// question worth answering afterwards was "when was this revoked", and the revocation wrote
/// the only record. A token whose scopes can grow needs the other end of the story too:
/// without the issuing event, the trail of a token that was widened starts at the widening
/// and never says what it was born with.
pub(crate) async fn create_token(
    connection: &mut SqliteConnection,
    id: CaptureTokenId,
    label: String,
    token_sha256: String,
    scopes: Vec<String>,
) -> Result<(CaptureToken, EventEnvelope)> {
    let token = CaptureToken {
        id,
        label,
        scopes,
        created_at: Utc::now(),
        last_used_at: None,
        revoked_at: None,
    };
    let event = EventEnvelope::new(
        EventKind::CaptureChanged,
        serde_json::json!({
            "capture_token_id": token.id,
            "issued": true,
            "scopes": token.scopes,
        }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO capture_tokens (id, label, token_sha256, scopes_json, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token.id.to_string())
    .bind(&token.label)
    .bind(token_sha256)
    .bind(serde_json::to_string(&token.scopes)?)
    .bind(token.created_at)
    .execute(&mut *transaction)
    .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((token, event))
}

/// Replaces the scopes of a live token without touching its digest.
///
/// The digest is what the bearer hashes to, so leaving it alone is the whole point: the
/// client keeps the value it already has and simply reaches more, or less, on its next
/// request. The previous scopes ride along in the event because the interesting question
/// afterwards is not what a token holds now — the list shows that — but what it gained and
/// when.
pub(crate) async fn update_token_scopes(
    connection: &mut SqliteConnection,
    id: CaptureTokenId,
    scopes: Vec<String>,
) -> Result<(CaptureToken, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let previous: CaptureToken = sqlx::query_as::<_, CaptureTokenRow>(
        "SELECT id, label, scopes_json, created_at, last_used_at, revoked_at FROM capture_tokens \
         WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(id.to_string())
    .fetch_optional(&mut *transaction)
    .await?
    .context(StoreError::not_found("capture token not found"))?
    .try_into()?;
    let token = CaptureToken {
        scopes,
        ..previous.clone()
    };
    sqlx::query("UPDATE capture_tokens SET scopes_json = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(serde_json::to_string(&token.scopes)?)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    let event = EventEnvelope::new(
        EventKind::CaptureChanged,
        serde_json::json!({
            "capture_token_id": id,
            "scopes_changed": true,
            "scopes": token.scopes,
            "previous_scopes": previous.scopes,
        }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((token, event))
}

pub(crate) async fn token_valid_with_scope(
    pool: &SqlitePool,
    token_sha256: &str,
    scope: &str,
) -> Result<bool> {
    let Some(scopes) = token_scopes(pool, token_sha256).await? else {
        return Ok(false);
    };
    Ok(rd_core::scopes_satisfy(
        scopes.iter().map(String::as_str),
        scope,
    ))
}

/// The scopes a live token holds, or `None` when no live token has that digest.
///
/// Distinct from [`token_valid_with_scope`] because the scope policy needs to say *what* a
/// token is missing, not only that something is. "This token may only read status resources"
/// is a message somebody can act on; a bare 403 is not.
pub(crate) async fn token_scopes(
    pool: &SqlitePool,
    token_sha256: &str,
) -> Result<Option<Vec<String>>> {
    let scopes_json = sqlx::query_scalar::<_, String>(
        "SELECT scopes_json FROM capture_tokens WHERE token_sha256 = ? AND revoked_at IS NULL",
    )
    .bind(token_sha256)
    .fetch_optional(pool)
    .await?;
    let Some(scopes_json) = scopes_json else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&scopes_json)?))
}

/// Who a live token is, plus what it holds, in the one query the policy check already runs.
///
/// The audit log names an actor (RD-110-03), and "the bearer whose SHA-256 begins 3f9c" is not
/// a name anybody can act on. The id and the label are both things a person already sees in
/// the token list and neither is a credential — the bearer value itself is not stored at all,
/// only its digest, and that digest does not leave this function.
pub(crate) async fn token_identity(
    pool: &SqlitePool,
    token_sha256: &str,
) -> Result<Option<(CaptureTokenId, String, Vec<String>)>> {
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, label, scopes_json FROM capture_tokens \
         WHERE token_sha256 = ? AND revoked_at IS NULL",
    )
    .bind(token_sha256)
    .fetch_optional(pool)
    .await?;
    let Some((id, label, scopes_json)) = row else {
        return Ok(None);
    };
    let Ok(id) = id.parse::<CaptureTokenId>() else {
        return Ok(None);
    };
    Ok(Some((id, label, serde_json::from_str(&scopes_json)?)))
}

pub(crate) async fn revoke_token(
    connection: &mut SqliteConnection,
    id: CaptureTokenId,
) -> Result<EventEnvelope> {
    let event = EventEnvelope::new(
        EventKind::CaptureChanged,
        serde_json::json!({ "capture_token_id": id, "revoked": true }),
    );
    let mut transaction = connection.begin().await?;
    let result =
        sqlx::query("UPDATE capture_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    (result.rows_affected() == 1)
        .then_some(())
        .context(StoreError::not_found("capture token not found"))?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Lists unrevoked tokens holding any of the given scopes, newest first.
///
/// Exact membership, not [`rd_core::scope_satisfies`]: the caller decides which scopes make
/// up one management surface, so the API token list can show `api:*` and `api:read` side by
/// side without a read-only token also appearing under capture agents.
pub(crate) async fn list_tokens(pool: &SqlitePool, scopes: &[&str]) -> Result<Vec<CaptureToken>> {
    let tokens: Vec<CaptureToken> = sqlx::query_as::<_, CaptureTokenRow>(
        "SELECT id, label, scopes_json, created_at, last_used_at, revoked_at FROM capture_tokens \
         WHERE revoked_at IS NULL ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect::<Result<_>>()?;
    Ok(tokens
        .into_iter()
        .filter(|token| {
            token
                .scopes
                .iter()
                .any(|held| scopes.contains(&held.as_str()))
        })
        .collect())
}

#[derive(FromRow)]
struct CaptureTokenRow {
    id: String,
    label: String,
    scopes_json: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl TryFrom<CaptureTokenRow> for CaptureToken {
    type Error = anyhow::Error;

    fn try_from(row: CaptureTokenRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            label: row.label,
            scopes: serde_json::from_str(&row.scopes_json)?,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            revoked_at: row.revoked_at,
        })
    }
}
