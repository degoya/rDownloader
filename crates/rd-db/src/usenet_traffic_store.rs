//! Traffic per Usenet server and the quota it is counted against (RD-1100-05).
//!
//! See `migrations/0123_usenet_server_traffic.sql` for the table. The download path counts the
//! bytes each server delivers in memory and hands them over here in one batch per flush, so the
//! writer sees one command every few seconds, never one per article. A flush is one
//! transaction: the day rows, the quota figures and the crossing of a limit are all written or
//! none of them is.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rd_core::{EventEnvelope, EventKind, UsenetQuota, UsenetServerId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{parse_id, usenet_store::QuotaColumns, writer::insert_event};

/// The bytes one server delivered over the usual ranges, and its quota.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsenetServerTraffic {
    pub server_id: UsenetServerId,
    pub name: String,
    pub enabled: bool,
    /// Since midnight UTC.
    pub today: u64,
    /// Today and the six days before it.
    pub week: u64,
    /// Today and the 29 days before it.
    pub month: u64,
    /// Today and the 364 days before it.
    pub year: u64,
    /// Everything recorded for the server.
    pub total: u64,
    pub quota: Option<UsenetQuota>,
}

/// A server whose quota one flush used up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsenetQuotaReached {
    pub server_id: UsenetServerId,
    pub name: String,
}

/// Adds one flush of counted bytes to the day rows and the quota figures.
///
/// A server deleted since its bytes were counted is skipped rather than failing the flush:
/// its rows went with it, and the other servers' bytes must not be lost for its sake. A due
/// reset day puts the used figure back to zero before the new bytes are added to it. A limit
/// the figure reaches for the first time is marked, and announced with a `usenet.changed`
/// event in the same transaction, which is what the notification hangs off; a limit already
/// marked is not announced again.
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    counts: &[(UsenetServerId, u64)],
    now: DateTime<Utc>,
) -> Result<(Vec<UsenetQuotaReached>, Vec<EventEnvelope>)> {
    let today = now.date_naive();
    let day = today.to_string();
    let mut reached = Vec::new();
    let mut events = Vec::new();
    let mut tx = connection.begin().await?;
    for (server_id, bytes) in counts {
        if *bytes == 0 {
            continue;
        }
        let id = server_id.to_string();
        let bytes = i64::try_from(*bytes).unwrap_or(i64::MAX);
        sqlx::query(
            "UPDATE usenet_servers SET quota_used_bytes = 0, quota_reached_at = NULL, \
             quota_reset_on = NULL WHERE id = ? AND quota_reset_on IS NOT NULL \
             AND quota_reset_on <= ?",
        )
        .bind(&id)
        .bind(&day)
        .execute(&mut *tx)
        .await
        .context("apply a due quota reset")?;
        let counted = sqlx::query(
            "INSERT INTO usenet_server_traffic (server_id, day, bytes) \
             SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM usenet_servers WHERE id = ?) \
             ON CONFLICT(server_id, day) DO UPDATE SET bytes = bytes + excluded.bytes",
        )
        .bind(&id)
        .bind(&day)
        .bind(bytes)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .context("record usenet server traffic")?
        .rows_affected();
        if counted == 0 {
            continue;
        }
        sqlx::query(
            "UPDATE usenet_servers SET quota_used_bytes = quota_used_bytes + ? \
             WHERE id = ? AND quota_bytes IS NOT NULL",
        )
        .bind(bytes)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .context("count usenet quota")?;
        let crossed = sqlx::query(
            "UPDATE usenet_servers SET quota_reached_at = ? WHERE id = ? \
             AND quota_bytes IS NOT NULL AND quota_reached_at IS NULL \
             AND quota_used_bytes >= quota_bytes",
        )
        .bind(now)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .context("mark a used-up usenet quota")?
        .rows_affected();
        if crossed == 0 {
            continue;
        }
        let (name, limit, action): (String, i64, String) = sqlx::query_as(
            "SELECT name, quota_bytes, quota_action FROM usenet_servers WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        let event = EventEnvelope::new(
            EventKind::UsenetChanged,
            serde_json::json!({
                "resource": "usenet_quota",
                "id": server_id,
                "name": name,
                "quota_reached": true,
                "limit_bytes": limit,
                "action": action,
            }),
        );
        insert_event(&mut tx, &event).await?;
        events.push(event);
        reached.push(UsenetQuotaReached {
            server_id: *server_id,
            name,
        });
    }
    tx.commit().await?;
    Ok((reached, events))
}

#[derive(FromRow)]
struct TrafficRow {
    id: String,
    name: String,
    enabled: bool,
    today: i64,
    week: i64,
    month: i64,
    year: i64,
    total: i64,
    #[sqlx(flatten)]
    quota: QuotaColumns,
}

/// Every configured server with its traffic as of `today`, in priority order.
pub(crate) async fn list(pool: &SqlitePool, today: NaiveDate) -> Result<Vec<UsenetServerTraffic>> {
    let since = |days: i64| (today - Duration::days(days)).to_string();
    let rows = sqlx::query_as::<_, TrafficRow>(
        "SELECT s.id, s.name, s.enabled, \
           COALESCE(SUM(CASE WHEN t.day >= ? THEN t.bytes END), 0) AS today, \
           COALESCE(SUM(CASE WHEN t.day >= ? THEN t.bytes END), 0) AS week, \
           COALESCE(SUM(CASE WHEN t.day >= ? THEN t.bytes END), 0) AS month, \
           COALESCE(SUM(CASE WHEN t.day >= ? THEN t.bytes END), 0) AS year, \
           COALESCE(SUM(t.bytes), 0) AS total, \
           s.quota_bytes, s.quota_action, s.quota_reset_on, s.quota_used_bytes, \
           s.quota_reached_at \
         FROM usenet_servers s LEFT JOIN usenet_server_traffic t ON t.server_id = s.id \
         GROUP BY s.id ORDER BY s.priority, s.name",
    )
    .bind(since(0))
    .bind(since(6))
    .bind(since(29))
    .bind(since(364))
    .fetch_all(pool)
    .await
    .context("list usenet server traffic")?;
    let figure = |value: i64| u64::try_from(value).unwrap_or(0);
    rows.into_iter()
        .map(|row| {
            Ok(UsenetServerTraffic {
                server_id: parse_id(&row.id)?,
                name: row.name,
                enabled: row.enabled,
                today: figure(row.today),
                week: figure(row.week),
                month: figure(row.month),
                year: figure(row.year),
                total: figure(row.total),
                quota: row.quota.quota(today),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "usenet_traffic_store_tests.rs"]
mod tests;
