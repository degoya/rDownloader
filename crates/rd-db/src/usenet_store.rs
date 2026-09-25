use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind, ProxyProfileId, UsenetServer, UsenetServerId};
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
    let value = UsenetServer {
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
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
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
    sqlx::query_as::<_, ServerRow>(
        "SELECT id, name, host, port, tls, username, password_ref IS NOT NULL AS has_password, \
         proxy_profile_id, priority, max_connections, enabled FROM usenet_servers \
         ORDER BY priority, name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn connection_config(
    pool: &SqlitePool,
    id: UsenetServerId,
) -> Result<Option<UsenetConnectionConfig>> {
    sqlx::query_as::<_, ConnectionRow>(
        "SELECT id, name, host, port, tls, username, password_ref, proxy_profile_id, \
         priority, max_connections, enabled FROM usenet_servers WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
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
}

impl TryFrom<ConnectionRow> for UsenetConnectionConfig {
    type Error = anyhow::Error;

    fn try_from(row: ConnectionRow) -> Result<Self> {
        let server = UsenetServer {
            id: parse_id(&row.id)?,
            name: row.name,
            host: row.host,
            port: u16::try_from(row.port)?,
            tls: row.tls,
            username: row.username,
            has_password: row.password_ref.is_some(),
            proxy_profile_id: row.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            priority: row.priority,
            max_connections: u16::try_from(row.max_connections)?,
            enabled: row.enabled,
        };
        Ok(Self {
            server,
            password_ref: row.password_ref,
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
}

impl TryFrom<ServerRow> for UsenetServer {
    type Error = anyhow::Error;

    fn try_from(row: ServerRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            host: row.host,
            port: u16::try_from(row.port)?,
            tls: row.tls,
            username: row.username,
            has_password: row.has_password,
            proxy_profile_id: row.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            priority: row.priority,
            max_connections: u16::try_from(row.max_connections)?,
            enabled: row.enabled,
        })
    }
}
