//! Persisted login sessions.
//!
//! Follows the `capture_tokens` shape: the bearer is never stored, only its SHA-256 digest,
//! so a database read — a backup, a copied file — yields no usable credential.
//!
//! Validation reads from the reader pool rather than going through the writer, because it
//! happens on every authenticated request and the writer is serialized. The one write on that
//! path, updating `last_used_at`, is deliberately *not* done every time: see
//! [`TOUCH_INTERVAL_SECONDS`].

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rd_core::{Session, SessionId, SessionLimits};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::parse_id;

/// How stale `last_used_at` may get before it is written again.
///
/// Writing it on every request would put one serialized write in front of every authenticated
/// call — an amplification the client controls — to make a timestamp accurate to the second.
/// The inventory exists to answer "is this still in use", and a minute's resolution answers
/// that as well as a second's.
pub const TOUCH_INTERVAL_SECONDS: i64 = 60;

pub(crate) async fn create_session(
    connection: &mut SqliteConnection,
    id: SessionId,
    token_sha256: String,
    user_agent: Option<String>,
    client_ip: Option<String>,
    lifetime_hours: i64,
) -> Result<Session> {
    let now = Utc::now();
    let session = Session {
        id,
        created_at: now,
        last_used_at: now,
        expires_at: now + Duration::hours(lifetime_hours),
        user_agent,
        client_ip,
        current: true,
    };
    sqlx::query(
        "INSERT INTO sessions (id, token_sha256, created_at, last_used_at, expires_at, \
         user_agent, client_ip) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session.id.to_string())
    .bind(token_sha256)
    .bind(session.created_at)
    .bind(session.last_used_at)
    .bind(session.expires_at)
    .bind(&session.user_agent)
    .bind(&session.client_ip)
    .execute(connection)
    .await
    .context("insert session")?;
    Ok(session)
}

/// The condition a live row meets, with its three parameters in the order [`live_bounds`]
/// returns them.
///
/// The limits are applied here, at read time, rather than written into `expires_at`, so a
/// shorter setting ends the sessions already past it on their next request (RD-130-09).
const LIVE: &str = "revoked_at IS NULL AND expires_at > ? AND created_at > ? AND last_used_at > ?";

/// The parameters of [`LIVE`]: now, the oldest sign-in and the oldest use still allowed.
fn live_bounds(limits: SessionLimits) -> [DateTime<Utc>; 3] {
    let now = Utc::now();
    [now, now - limits.max(), now - limits.idle()]
}

/// The live session behind a digest, or `None` if there is none.
///
/// "Live" means not revoked, not past the expiry fixed at sign-in, and inside both `limits`.
/// An expired row is left in place for [`purge_expired`] rather than deleted here: this runs
/// on the reader pool.
pub(crate) async fn session_for_digest(
    pool: &SqlitePool,
    token_sha256: &str,
    limits: SessionLimits,
) -> Result<Option<Session>> {
    let query = format!(
        "SELECT id, created_at, last_used_at, expires_at, user_agent, client_ip \
         FROM sessions WHERE token_sha256 = ? AND {LIVE}"
    );
    let [now, signed_in_after, used_after] = live_bounds(limits);
    let row = sqlx::query_as::<_, SessionRow>(&query)
        .bind(token_sha256)
        .bind(now)
        .bind(signed_in_after)
        .bind(used_after)
        .fetch_optional(pool)
        .await
        .context("read session")?;
    row.map(|row| live_session(row, limits)).transpose()
}

/// Advances `last_used_at`, and reports whether the row was still live.
pub(crate) async fn touch_session(
    connection: &mut SqliteConnection,
    token_sha256: &str,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE sessions SET last_used_at = ? WHERE token_sha256 = ? AND revoked_at IS NULL",
    )
    .bind(Utc::now())
    .bind(token_sha256)
    .execute(connection)
    .await
    .context("touch session")?;
    Ok(result.rows_affected() > 0)
}

/// Every live session under `limits`, newest first.
pub(crate) async fn list_sessions(
    pool: &SqlitePool,
    limits: SessionLimits,
) -> Result<Vec<Session>> {
    let query = format!(
        "SELECT id, created_at, last_used_at, expires_at, user_agent, client_ip \
         FROM sessions WHERE {LIVE} ORDER BY last_used_at DESC"
    );
    let [now, signed_in_after, used_after] = live_bounds(limits);
    let rows = sqlx::query_as::<_, SessionRow>(&query)
        .bind(now)
        .bind(signed_in_after)
        .bind(used_after)
        .fetch_all(pool)
        .await
        .context("list sessions")?;
    rows.into_iter()
        .map(|row| live_session(row, limits))
        .collect()
}

/// A live row as the inventory shows it: `expires_at` is when it ends under `limits` if it
/// is not used again, not the expiry fixed at sign-in, which a shorter setting overrides.
fn live_session(row: SessionRow, limits: SessionLimits) -> Result<Session> {
    let mut session = Session::try_from(row)?;
    session.expires_at = limits.ends_at(&session);
    Ok(session)
}

/// Revokes one session by id. Returns whether it was live.
pub(crate) async fn revoke_session(
    connection: &mut SqliteConnection,
    id: SessionId,
) -> Result<bool> {
    let result =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(connection)
            .await
            .context("revoke session")?;
    Ok(result.rows_affected() > 0)
}

/// Revokes every session except the one holding `keep_digest`.
///
/// Keeping the caller's own session is the whole point: "sign out everywhere else" that also
/// signs you out is a worse tool than no tool, because the person reaching for it is usually
/// reacting to something and now has to log in again to see whether it worked.
pub(crate) async fn revoke_other_sessions(
    connection: &mut SqliteConnection,
    keep_digest: &str,
) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = ? WHERE revoked_at IS NULL AND token_sha256 <> ?",
    )
    .bind(Utc::now())
    .bind(keep_digest)
    .execute(connection)
    .await
    .context("revoke other sessions")?;
    Ok(result.rows_affected())
}

/// Revokes every live session, the caller's own included. Returns how many ended.
///
/// Separate from [`revoke_other_sessions`] with an empty digest, which would do the same thing
/// by accident: this is the one write that is *meant* to sign the caller out too, and a reader
/// should not have to work out that `<> ''` matches everything to see it.
pub(crate) async fn revoke_all(connection: &mut SqliteConnection) -> Result<u64> {
    let result = sqlx::query("UPDATE sessions SET revoked_at = ? WHERE revoked_at IS NULL")
        .bind(Utc::now())
        .execute(connection)
        .await
        .context("revoke all sessions")?;
    Ok(result.rows_affected())
}

/// Deletes rows that ended under `limits` more than thirty days ago.
///
/// Measured against the same three bounds a read applies, so a session that ended by idling
/// is not kept for the rest of a maximum lifetime it will never reach.
pub(crate) async fn purge_expired(
    connection: &mut SqliteConnection,
    limits: SessionLimits,
) -> Result<u64> {
    let cutoff = Utc::now() - Duration::days(30);
    let result = sqlx::query(
        "DELETE FROM sessions WHERE expires_at < ? OR created_at < ? OR last_used_at < ?",
    )
    .bind(cutoff)
    .bind(cutoff - limits.max())
    .bind(cutoff - limits.idle())
    .execute(connection)
    .await
    .context("purge expired sessions")?;
    Ok(result.rows_affected())
}

/// Notes that a machine token was used, at most once per [`TOUCH_INTERVAL_SECONDS`].
pub(crate) async fn touch_capture_token(
    connection: &mut SqliteConnection,
    token_sha256: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE capture_tokens SET last_used_at = ? WHERE token_sha256 = ? \
         AND revoked_at IS NULL AND (last_used_at IS NULL OR last_used_at < ?)",
    )
    .bind(Utc::now())
    .bind(token_sha256)
    .bind(Utc::now() - Duration::seconds(TOUCH_INTERVAL_SECONDS))
    .execute(connection)
    .await
    .context("touch capture token")?;
    Ok(())
}

#[derive(FromRow)]
struct SessionRow {
    id: String,
    created_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    user_agent: Option<String>,
    client_ip: Option<String>,
}

impl TryFrom<SessionRow> for Session {
    type Error = anyhow::Error;

    fn try_from(row: SessionRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            expires_at: row.expires_at,
            user_agent: row.user_agent,
            client_ip: row.client_ip,
            // The caller decides; a row on its own does not know who is asking.
            current: false,
        })
    }
}
