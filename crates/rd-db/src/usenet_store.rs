use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rd_core::{
    EventEnvelope, EventKind, ProxyProfileId, UsenetQuota, UsenetQuotaAction, UsenetServer,
    UsenetServerId,
};
use sqlx::{Connection, FromRow, Row, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

#[derive(Clone, Debug)]
pub struct NewUsenetServer {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    pub password_ref: Option<String>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

pub type UpdateUsenetServer = NewUsenetServer;

/// Internal server metadata including the opaque vault reference.
pub struct UsenetConnectionConfig {
    pub server: UsenetServer,
    pub password_ref: Option<String>,
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewUsenetServer,
) -> Result<(UsenetServer, EventEnvelope)> {
    let value = UsenetServer {
        id: UsenetServerId::new(),
        name: input.name,
        host: input.host,
        port: input.port,
        tls: input.tls,
        username: input.username,
        has_password: input.password_ref.is_some(),
        proxy_profile_id: input.proxy_profile_id,
        priority: input.priority,
        max_connections: input.max_connections,
        enabled: input.enabled,
        quota: None,
    };
    let now = Utc::now();
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "resource": "usenet_server", "id": value.id }),
    );
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO usenet_servers (id, name, host, port, tls, username, password_ref, \
         proxy_profile_id, priority, max_connections, enabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(&value.host)
    .bind(i64::from(value.port))
    .bind(value.tls)
    .bind(&value.username)
    .bind(input.password_ref)
    .bind(value.proxy_profile_id.map(|id| id.to_string()))
    .bind(value.priority)
    .bind(i64::from(value.max_connections))
    .bind(value.enabled)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: UsenetServerId,
    input: UpdateUsenetServer,
) -> Result<(UsenetServer, EventEnvelope)> {
    let mut value = UsenetServer {
        id,
        name: input.name,
        host: input.host,
        port: input.port,
        tls: input.tls,
        username: input.username,
        has_password: input.password_ref.is_some(),
        proxy_profile_id: input.proxy_profile_id,
        priority: input.priority,
        max_connections: input.max_connections,
        enabled: input.enabled,
        quota: None,
    };
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "resource": "usenet_server", "id": value.id }),
    );
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE usenet_servers SET name = ?, host = ?, port = ?, tls = ?, username = ?, \
         password_ref = ?, proxy_profile_id = ?, priority = ?, max_connections = ?, enabled = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(&value.host)
    .bind(i64::from(value.port))
    .bind(value.tls)
    .bind(&value.username)
    .bind(input.password_ref)
    .bind(value.proxy_profile_id.map(|id| id.to_string()))
    .bind(value.priority)
    .bind(i64::from(value.max_connections))
    .bind(value.enabled)
    .bind(Utc::now())
    .bind(value.id.to_string())
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        StoreError::not_found("usenet server not found")
    );
    // The quota is not part of the edit; the server comes back with the one it has.
    value.quota = sqlx::query_as::<_, QuotaColumns>(sqlx::AssertSqlSafe(format!(
        "SELECT {QUOTA_COLUMNS} FROM usenet_servers WHERE id = ?"
    )))
    .bind(value.id.to_string())
    .fetch_one(&mut *tx)
    .await?
    .quota(Utc::now().date_naive());
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// What `set_quota` writes: the limit (`None` removes the quota), the action, the optional
/// reset day, and whether the used figure starts again at zero now.
#[derive(Clone, Debug)]
pub struct UsenetQuotaInput {
    pub limit_bytes: Option<u64>,
    pub action: UsenetQuotaAction,
    pub reset_on: Option<NaiveDate>,
    pub reset_usage: bool,
}

/// Sets, changes or removes the quota of one server (RD-1100-05).
///
/// The used figure counts only while a quota is set: a new quota starts at zero, and removing
/// one forgets the figure. A changed limit the figure already reaches counts as reached from
/// now on, without a notification: the person setting it is looking at the figure. A limit
/// above it clears the mark, so a raised quota gives the server back at once.
pub(crate) async fn set_quota(
    connection: &mut SqliteConnection,
    id: UsenetServerId,
    input: UsenetQuotaInput,
) -> Result<(UsenetServer, EventEnvelope)> {
    let now = Utc::now();
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "resource": "usenet_server", "id": id }),
    );
    let limit = input
        .limit_bytes
        .map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX));
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE usenet_servers SET quota_bytes = ?, quota_action = ?, quota_reset_on = ?, \
         quota_used_bytes = CASE WHEN ? OR ? IS NULL OR quota_bytes IS NULL THEN 0 \
           ELSE quota_used_bytes END, updated_at = ? \
         WHERE id = ?",
    )
    .bind(limit)
    .bind(input.action.as_str())
    .bind(input.reset_on.map(|day| day.to_string()))
    .bind(input.reset_usage)
    .bind(limit)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        StoreError::not_found("usenet server not found")
    );
    sqlx::query(
        "UPDATE usenet_servers SET quota_reached_at = CASE \
           WHEN quota_bytes IS NOT NULL AND quota_used_bytes >= quota_bytes \
             THEN COALESCE(quota_reached_at, ?) \
           ELSE NULL END \
         WHERE id = ?",
    )
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query_as::<_, ServerRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SERVER_COLUMNS} FROM usenet_servers WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let server = row.into_server(now.date_naive())?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((server, event))
}

pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: UsenetServerId,
) -> Result<(Option<String>, EventEnvelope)> {
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "resource": "usenet_server", "id": id, "removed": true }),
    );
    let mut tx = connection.begin().await?;
    let row = sqlx::query("SELECT password_ref FROM usenet_servers WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context(StoreError::not_found("usenet server not found"))?;
    sqlx::query("DELETE FROM usenet_servers WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((row.get("password_ref"), event))
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<UsenetServer>> {
    let today = Utc::now().date_naive();
    sqlx::query_as::<_, ServerRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SERVER_COLUMNS} FROM usenet_servers ORDER BY priority, name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| row.into_server(today))
    .collect()
}

pub(crate) async fn connection_config(
    pool: &SqlitePool,
    id: UsenetServerId,
) -> Result<Option<UsenetConnectionConfig>> {
    let today = Utc::now().date_naive();
    sqlx::query_as::<_, ConnectionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT id, name, host, port, tls, username, password_ref, proxy_profile_id, \
         priority, max_connections, enabled, {QUOTA_COLUMNS} FROM usenet_servers WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(|row| row.into_config(today))
    .transpose()
}

/// The quota columns, in the order [`QuotaColumns`] reads them.
pub(crate) const QUOTA_COLUMNS: &str =
    "quota_bytes, quota_action, quota_reset_on, quota_used_bytes, quota_reached_at";

/// Everything [`ServerRow`] reads.
const SERVER_COLUMNS: &str = "id, name, host, port, tls, username, \
     password_ref IS NOT NULL AS has_password, proxy_profile_id, priority, max_connections, \
     enabled, quota_bytes, quota_action, quota_reset_on, quota_used_bytes, quota_reached_at";

/// The stored quota of one server, before the reset day is applied.
#[derive(FromRow)]
pub(crate) struct QuotaColumns {
    quota_bytes: Option<i64>,
    quota_action: String,
    quota_reset_on: Option<String>,
    quota_used_bytes: i64,
    quota_reached_at: Option<DateTime<Utc>>,
}

impl QuotaColumns {
    /// The quota as of `today`: a reset day that has come means nothing is used yet.
    ///
    /// The stored figure is put back to zero by the next flush of the server's traffic. Until
    /// then every reader applies the reset itself, so a server paused by its quota comes back
    /// on its reset day even though, paused, it has no traffic that would flush.
    pub(crate) fn quota(self, today: NaiveDate) -> Option<UsenetQuota> {
        let limit = self.quota_bytes?;
        let reset_on = self
            .quota_reset_on
            .as_deref()
            .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok());
        let due = reset_on.is_some_and(|day| day <= today);
        Some(UsenetQuota {
            limit_bytes: u64::try_from(limit).unwrap_or(0),
            action: UsenetQuotaAction::from_stored(&self.quota_action),
            reset_on: if due { None } else { reset_on },
            used_bytes: if due {
                0
            } else {
                u64::try_from(self.quota_used_bytes).unwrap_or(0)
            },
            reached_at: if due { None } else { self.quota_reached_at },
        })
    }
}

#[derive(FromRow)]
struct ConnectionRow {
    id: String,
    name: String,
    host: String,
    port: i64,
    tls: bool,
    username: Option<String>,
    password_ref: Option<String>,
    proxy_profile_id: Option<String>,
    priority: i32,
    max_connections: i64,
    enabled: bool,
    #[sqlx(flatten)]
    quota: QuotaColumns,
}

impl ConnectionRow {
    fn into_config(self, today: NaiveDate) -> Result<UsenetConnectionConfig> {
        let server = UsenetServer {
            id: parse_id(&self.id)?,
            name: self.name,
            host: self.host,
            port: u16::try_from(self.port)?,
            tls: self.tls,
            username: self.username,
            has_password: self.password_ref.is_some(),
            proxy_profile_id: self.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            priority: self.priority,
            max_connections: u16::try_from(self.max_connections)?,
            enabled: self.enabled,
            quota: self.quota.quota(today),
        };
        Ok(UsenetConnectionConfig {
            server,
            password_ref: self.password_ref,
        })
    }
}

#[derive(FromRow)]
struct ServerRow {
    id: String,
    name: String,
    host: String,
    port: i64,
    tls: bool,
    username: Option<String>,
    has_password: bool,
    proxy_profile_id: Option<String>,
    priority: i32,
    max_connections: i64,
    enabled: bool,
    #[sqlx(flatten)]
    quota: QuotaColumns,
}

impl ServerRow {
    fn into_server(self, today: NaiveDate) -> Result<UsenetServer> {
        Ok(UsenetServer {
            id: parse_id(&self.id)?,
            name: self.name,
            host: self.host,
            port: u16::try_from(self.port)?,
            tls: self.tls,
            username: self.username,
            has_password: self.has_password,
            proxy_profile_id: self.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            priority: self.priority,
            max_connections: u16::try_from(self.max_connections)?,
            enabled: self.enabled,
            quota: self.quota.quota(today),
        })
    }
}
