//! Persistence of notification targets, rules and the delivery history.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    EventEnvelope, EventKind, NotificationDeliveryId, NotificationRuleId, NotificationTargetId,
};
use rd_notify::{
    Delivery, DeliveryState, NotificationEvent, NotificationRule, NotificationTarget, Severity,
    TargetKind,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

/// Deliveries kept per rule. Enough to see what a rule has been doing lately, small enough
/// that the history can never become a disk-space bug; older entries are dropped on insert.
pub(crate) const MAX_DELIVERIES_PER_RULE: i64 = 200;

/// Editable fields of a target; the id stays with the row.
#[derive(Clone, Debug)]
pub struct NewNotificationTarget {
    pub name: String,
    pub kind: TargetKind,
    pub enabled: bool,
    pub endpoint: String,
    pub config: serde_json::Value,
    /// `None` keeps whatever is stored; use `clear_secret` to remove it.
    pub secret_ref: Option<String>,
    pub clear_secret: bool,
}

#[derive(Clone, Debug)]
pub struct NewNotificationRule {
    pub name: String,
    pub enabled: bool,
    pub target_id: NotificationTargetId,
    pub events: Vec<NotificationEvent>,
    pub category_id: Option<rd_core::CategoryId>,
    pub min_severity: Severity,
}

/// A delivery about to be queued.
#[derive(Clone, Debug)]
pub struct NewDelivery {
    pub rule_id: NotificationRuleId,
    pub target_id: NotificationTargetId,
    pub idempotency_key: String,
    pub event: NotificationEvent,
    pub title: String,
    pub body: String,
}

#[derive(FromRow)]
struct TargetRow {
    id: String,
    name: String,
    kind: String,
    enabled: bool,
    endpoint: String,
    config_json: String,
    secret_ref: Option<String>,
}

impl TryFrom<TargetRow> for NotificationTarget {
    type Error = anyhow::Error;

    fn try_from(row: TargetRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            kind: parse_json(&row.kind).context("stored target kind")?,
            enabled: row.enabled,
            endpoint: row.endpoint,
            config: serde_json::from_str(&row.config_json).context("stored target config")?,
            has_secret: row.secret_ref.is_some(),
            secret_ref: row.secret_ref,
        })
    }
}

#[derive(FromRow)]
struct RuleRow {
    id: String,
    name: String,
    enabled: bool,
    target_id: String,
    events_json: String,
    category_id: Option<String>,
    min_severity: String,
}

impl TryFrom<RuleRow> for NotificationRule {
    type Error = anyhow::Error;

    fn try_from(row: RuleRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            enabled: row.enabled,
            target_id: parse_id(&row.target_id)?,
            events: serde_json::from_str(&row.events_json).context("stored rule events")?,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            min_severity: parse_json(&row.min_severity).context("stored severity")?,
        })
    }
}

#[derive(FromRow)]
struct DeliveryRow {
    id: String,
    rule_id: String,
    target_id: String,
    idempotency_key: String,
    event: String,
    title: String,
    body: String,
    state: String,
    attempt: i64,
    next_attempt_at: Option<DateTime<Utc>>,
    response_status: Option<i64>,
    response_excerpt: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<DeliveryRow> for Delivery {
    type Error = anyhow::Error;

    fn try_from(row: DeliveryRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            rule_id: parse_id(&row.rule_id)?,
            target_id: parse_id(&row.target_id)?,
            idempotency_key: row.idempotency_key,
            event: parse_json(&row.event).context("stored delivery event")?,
            title: row.title,
            body: row.body,
            state: parse_json(&row.state).context("stored delivery state")?,
            attempt: u32::try_from(row.attempt).unwrap_or_default(),
            next_attempt_at: row.next_attempt_at,
            response_status: row
                .response_status
                .and_then(|value| u16::try_from(value).ok()),
            response_excerpt: row.response_excerpt,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// The enums are stored as their serde names, so the column reads like the API does.
fn parse_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    Ok(serde_json::from_str(&format!("\"{value}\""))?)
}

fn to_name<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?.trim_matches('"').to_owned())
}

const TARGET_COLUMNS: &str = "id, name, kind, enabled, endpoint, config_json, secret_ref";
const RULE_COLUMNS: &str = "id, name, enabled, target_id, events_json, category_id, min_severity";
const DELIVERY_COLUMNS: &str = "id, rule_id, target_id, idempotency_key, event, title, body, \
     state, attempt, next_attempt_at, response_status, response_excerpt, created_at, updated_at";

pub(crate) async fn list_targets(pool: &SqlitePool) -> Result<Vec<NotificationTarget>> {
    sqlx::query_as::<_, TargetRow>(&format!(
        "SELECT {TARGET_COLUMNS} FROM notification_targets ORDER BY name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn list_rules(pool: &SqlitePool) -> Result<Vec<NotificationRule>> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {RULE_COLUMNS} FROM notification_rules ORDER BY name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Newest deliveries first; the read is paginated, and the history itself is bounded by
/// [`MAX_DELIVERIES_PER_RULE`].
pub(crate) async fn list_deliveries(pool: &SqlitePool, limit: u32) -> Result<Vec<Delivery>> {
    sqlx::query_as::<_, DeliveryRow>(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM notification_deliveries ORDER BY created_at DESC LIMIT ?"
    ))
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// How many deliveries [`clear_deliveries`] would remove right now.
pub(crate) async fn count_clearable_deliveries(pool: &SqlitePool) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_deliveries WHERE state NOT IN ('queued', 'retrying')",
    )
    .fetch_one(pool)
    .await
    .context("count notification deliveries")?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// Deliveries the worker should attempt now.
pub(crate) async fn due_deliveries(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<Delivery>> {
    sqlx::query_as::<_, DeliveryRow>(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM notification_deliveries \
         WHERE state IN ('queued', 'retrying') AND (next_attempt_at IS NULL OR next_attempt_at <= ?) \
         ORDER BY created_at LIMIT 50"
    ))
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn upsert_target(
    connection: &mut SqliteConnection,
    id: Option<NotificationTargetId>,
    input: NewNotificationTarget,
) -> Result<(NotificationTarget, EventEnvelope)> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let id = match id {
        Some(id) => {
            // An omitted secret keeps the stored one; `clear_secret` removes it explicitly.
            let secret_sql = if input.clear_secret {
                "secret_ref = NULL"
            } else if input.secret_ref.is_some() {
                "secret_ref = ?"
            } else {
                "secret_ref = secret_ref"
            };
            let statement = format!(
                "UPDATE notification_targets SET name = ?, kind = ?, enabled = ?, endpoint = ?, \
                 config_json = ?, {secret_sql}, updated_at = ? WHERE id = ?"
            );
            let mut query = sqlx::query(&statement)
                .bind(&input.name)
                .bind(to_name(&input.kind)?)
                .bind(input.enabled)
                .bind(&input.endpoint)
                .bind(serde_json::to_string(&input.config)?);
            if !input.clear_secret && input.secret_ref.is_some() {
                query = query.bind(input.secret_ref.clone());
            }
            let updated = query
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            anyhow::ensure!(
                updated.rows_affected() > 0,
                StoreError::not_found("notification target not found")
            );
            id
        }
        None => {
            let id = NotificationTargetId::new();
            sqlx::query(
                "INSERT INTO notification_targets (id, name, kind, enabled, endpoint, \
                 config_json, secret_ref, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(&input.name)
            .bind(to_name(&input.kind)?)
            .bind(input.enabled)
            .bind(&input.endpoint)
            .bind(serde_json::to_string(&input.config)?)
            .bind(input.secret_ref.clone())
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    let row = sqlx::query_as::<_, TargetRow>(&format!(
        "SELECT {TARGET_COLUMNS} FROM notification_targets WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let event = changed_event("target", id.to_string());
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((row.try_into()?, event))
}

pub(crate) async fn delete_target(
    connection: &mut SqliteConnection,
    id: NotificationTargetId,
) -> Result<(Option<String>, EventEnvelope)> {
    let event = changed_event("target", id.to_string());
    let mut tx = connection.begin().await?;
    let secret_ref: Option<Option<String>> =
        sqlx::query_scalar("SELECT secret_ref FROM notification_targets WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    let secret_ref = secret_ref.context(StoreError::not_found("notification target not found"))?;
    // Rules without a target could never deliver, so they go with it.
    sqlx::query("DELETE FROM notification_rules WHERE target_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM notification_targets WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((secret_ref, event))
}

pub(crate) async fn upsert_rule(
    connection: &mut SqliteConnection,
    id: Option<NotificationRuleId>,
    input: NewNotificationRule,
) -> Result<(NotificationRule, EventEnvelope)> {
    let value = NotificationRule {
        id: id.unwrap_or_default(),
        name: input.name,
        enabled: input.enabled,
        target_id: input.target_id,
        events: input.events,
        category_id: input.category_id,
        min_severity: input.min_severity,
    };
    let now = Utc::now();
    let event = changed_event("rule", value.id.to_string());
    let mut tx = connection.begin().await?;
    let events_json = serde_json::to_string(&value.events)?;
    let severity = to_name(&value.min_severity)?;
    if id.is_some() {
        let updated = sqlx::query(
            "UPDATE notification_rules SET name = ?, enabled = ?, target_id = ?, \
             events_json = ?, category_id = ?, min_severity = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&value.name)
        .bind(value.enabled)
        .bind(value.target_id.to_string())
        .bind(&events_json)
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(&severity)
        .bind(now)
        .bind(value.id.to_string())
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            updated.rows_affected() > 0,
            StoreError::not_found("notification rule not found")
        );
    } else {
        sqlx::query(
            "INSERT INTO notification_rules (id, name, enabled, target_id, events_json, \
             category_id, min_severity, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(value.enabled)
        .bind(value.target_id.to_string())
        .bind(&events_json)
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(&severity)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_rule(
    connection: &mut SqliteConnection,
    id: NotificationRuleId,
) -> Result<EventEnvelope> {
    let event = changed_event("rule", id.to_string());
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM notification_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    anyhow::ensure!(
        deleted.rows_affected() > 0,
        StoreError::not_found("notification rule not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Queues a delivery. A key that already exists is left alone, which is what makes one
/// event produce at most one delivery per rule.
pub(crate) async fn queue_delivery(
    connection: &mut SqliteConnection,
    input: NewDelivery,
) -> Result<bool> {
    let now = Utc::now();
    let inserted = sqlx::query(
        "INSERT INTO notification_deliveries (id, rule_id, target_id, idempotency_key, event, \
         title, body, state, attempt, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', 0, ?, ?) \
         ON CONFLICT(idempotency_key) DO NOTHING",
    )
    .bind(NotificationDeliveryId::new().to_string())
    .bind(input.rule_id.to_string())
    .bind(input.target_id.to_string())
    .bind(&input.idempotency_key)
    .bind(to_name(&input.event)?)
    .bind(&input.title)
    .bind(&input.body)
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if inserted.rows_affected() > 0 {
        // Trim in the same write, so the table cannot grow between two inserts. Rows the
        // worker still owes an attempt are held back: dropping one of those would discard the
        // notification itself rather than only its record.
        sqlx::query(
            "DELETE FROM notification_deliveries WHERE rule_id = ? \
             AND state NOT IN ('queued', 'retrying') AND id NOT IN ( \
               SELECT id FROM notification_deliveries WHERE rule_id = ? \
               ORDER BY created_at DESC, rowid DESC LIMIT ? )",
        )
        .bind(input.rule_id.to_string())
        .bind(input.rule_id.to_string())
        .bind(MAX_DELIVERIES_PER_RULE)
        .execute(&mut *connection)
        .await?;
    }
    Ok(inserted.rows_affected() > 0)
}

/// Empties the delivery history on request and reports how many rows went (RD-130-08).
///
/// Rows in `queued` or `retrying` stay, by the rule the per-rule trim in [`queue_delivery`]
/// follows: the worker still owes them an attempt, and deleting one would discard the
/// notification itself rather than only its record.
pub(crate) async fn clear_deliveries(connection: &mut SqliteConnection) -> Result<u64> {
    let deleted = sqlx::query(
        "DELETE FROM notification_deliveries WHERE state NOT IN ('queued', 'retrying')",
    )
    .execute(&mut *connection)
    .await
    .context("clear notification deliveries")?;
    Ok(deleted.rows_affected())
}

/// Records the outcome of one attempt.
pub(crate) async fn record_attempt(
    connection: &mut SqliteConnection,
    id: NotificationDeliveryId,
    state: DeliveryState,
    attempt: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    response_status: Option<u16>,
    response_excerpt: Option<String>,
) -> Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET state = ?, attempt = ?, next_attempt_at = ?, \
         response_status = ?, response_excerpt = ?, updated_at = ? WHERE id = ?",
    )
    .bind(to_name(&state)?)
    .bind(i64::from(attempt))
    .bind(next_attempt_at)
    .bind(response_status.map(i64::from))
    .bind(response_excerpt)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn changed_event(entity: &str, id: String) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::NotificationChanged,
        serde_json::json!({ "entity": entity, "id": id }),
    )
}
