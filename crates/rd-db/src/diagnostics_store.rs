//! What the diagnostic bundle reads about notifications and the queue (RD-1291-04).
//!
//! Every read here selects the columns it may hand on and nothing else: an endpoint, a token,
//! a title, a text, a package folder, a package or file name and an address are never in a
//! `SELECT` list, so no later step has to remember to drop them. Ids, states, stages, events,
//! error codes and counts are what a reader needs to tell where a notification or a package
//! stopped, and none of them says what was downloaded or where it was announced.
//!
//! The values stay the stored strings rather than the domain enums: a bundle is read when
//! something is wrong, and a row an enum no longer parses must still be shown, not refused.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

/// A notification target without where it sends to: no endpoint, no topic, no secret.
#[derive(Clone, Debug, Default, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct DiagnosticTarget {
    pub id: String,
    pub kind: String,
    pub enabled: bool,
    /// The destination plugin of a `plugin` target; `None` for every other kind.
    pub plugin_id: Option<String>,
}

/// A notification rule without its name and its wording: a template is only *set or not*.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct DiagnosticRule {
    pub id: String,
    pub target_id: String,
    pub enabled: bool,
    /// Empty means every event except the opt-in ones, as the rule itself reads it.
    pub events: Vec<String>,
    pub category_id: Option<String>,
    pub min_severity: String,
    pub has_title_template: bool,
    pub has_body_template: bool,
}

/// How many deliveries of one event ended in one state.
#[derive(Clone, Debug, Default, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct DeliveryCount {
    pub event: String,
    pub state: String,
    pub count: u64,
}

/// One delivery without its title, its text, its package folder and the response's text.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct DiagnosticDelivery {
    pub rule_id: String,
    pub event: String,
    pub state: String,
    pub attempts: u32,
    pub response_status: Option<u16>,
    pub queued_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Targets, rules and what became of their deliveries.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct NotificationDiagnostics {
    pub targets: Vec<DiagnosticTarget>,
    pub rules: Vec<DiagnosticRule>,
    /// Deliveries queued since the window's start, per event and state.
    pub delivery_counts: Vec<DeliveryCount>,
    /// The newest deliveries first.
    pub recent_deliveries: Vec<DiagnosticDelivery>,
}

/// How many rows hold one value: packages per state, packages per post-processing stage.
#[derive(Clone, Debug, Default, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct ValueCount {
    pub value: String,
    pub count: u64,
}

/// How many downloads are in one state with one recorded failure.
#[derive(Clone, Debug, Default, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct DownloadCount {
    pub state: String,
    /// The failure's class (`permanent`, `offline`, ...); `None` without a recorded failure.
    pub error_kind: Option<String>,
    /// The failure's stable code; `None` when there is none or it carried none.
    pub error_code: Option<String>,
    pub count: u64,
}

/// A package changing state, by id: never its name or its folder.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PackageTransition {
    pub package_id: String,
    /// `None` for a package that had no state before, such as one an NZB just queued.
    pub previous: Option<String>,
    pub state: String,
    pub stage: Option<String>,
    /// The package's category now; `None` without one or once the package is gone.
    pub category_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// Where the queue's packages and downloads stand.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct QueueDiagnostics {
    pub packages_by_state: Vec<ValueCount>,
    /// Packages that carry a post-processing stage, per stage.
    pub packages_by_stage: Vec<ValueCount>,
    /// The newest transitions first.
    pub recent_transitions: Vec<PackageTransition>,
    pub downloads: Vec<DownloadCount>,
}

#[derive(FromRow)]
struct RuleRow {
    id: String,
    target_id: String,
    enabled: bool,
    events_json: String,
    category_id: Option<String>,
    min_severity: String,
    has_title_template: i64,
    has_body_template: i64,
}

#[derive(FromRow)]
struct DeliveryRow {
    rule_id: String,
    event: String,
    state: String,
    attempt: i64,
    response_status: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct CountRow {
    event: String,
    state: String,
    count: i64,
}

#[derive(FromRow)]
struct ValueRow {
    value: String,
    count: i64,
}

#[derive(FromRow)]
struct DownloadRow {
    state: String,
    error_kind: Option<String>,
    error_code: Option<String>,
    count: i64,
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Targets, rules, the deliveries queued since `since` per event and state, and the newest
/// `recent` deliveries.
pub(crate) async fn notification_diagnostics(
    pool: &SqlitePool,
    since: DateTime<Utc>,
    recent: u32,
) -> Result<NotificationDiagnostics> {
    let targets = sqlx::query_as::<_, DiagnosticTarget>(
        "SELECT id, kind, enabled, \
         CASE WHEN kind = 'plugin' AND json_valid(config_json) \
              THEN json_extract(config_json, '$.plugin_id') END AS plugin_id \
         FROM notification_targets ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("read notification targets for diagnostics")?;
    let rules = sqlx::query_as::<_, RuleRow>(
        "SELECT id, target_id, enabled, events_json, category_id, min_severity, \
         COALESCE(title_template, '') <> '' AS has_title_template, \
         COALESCE(body_template, '') <> '' AS has_body_template \
         FROM notification_rules ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("read notification rules for diagnostics")?
    .into_iter()
    .map(|row| DiagnosticRule {
        id: row.id,
        target_id: row.target_id,
        enabled: row.enabled,
        // A list that does not parse reads as empty rather than failing the whole bundle.
        events: serde_json::from_str(&row.events_json).unwrap_or_default(),
        category_id: row.category_id,
        min_severity: row.min_severity,
        has_title_template: row.has_title_template != 0,
        has_body_template: row.has_body_template != 0,
    })
    .collect();
    let delivery_counts = sqlx::query_as::<_, CountRow>(
        "SELECT event, state, COUNT(*) AS count FROM notification_deliveries \
         WHERE created_at >= ? GROUP BY event, state ORDER BY event, state",
    )
    .bind(since)
    .fetch_all(pool)
    .await
    .context("count notification deliveries for diagnostics")?
    .into_iter()
    .map(|row| DeliveryCount {
        event: row.event,
        state: row.state,
        count: count(row.count),
    })
    .collect();
    let recent_deliveries = sqlx::query_as::<_, DeliveryRow>(
        "SELECT rule_id, event, state, attempt, response_status, created_at, updated_at \
         FROM notification_deliveries ORDER BY created_at DESC, rowid DESC LIMIT ?",
    )
    .bind(i64::from(recent))
    .fetch_all(pool)
    .await
    .context("read notification deliveries for diagnostics")?
    .into_iter()
    .map(|row| DiagnosticDelivery {
        rule_id: row.rule_id,
        event: row.event,
        state: row.state,
        attempts: u32::try_from(row.attempt).unwrap_or_default(),
        response_status: row
            .response_status
            .and_then(|value| u16::try_from(value).ok()),
        queued_at: row.created_at,
        updated_at: row.updated_at,
    })
    .collect();
    Ok(NotificationDiagnostics {
        targets,
        rules,
        delivery_counts,
        recent_deliveries,
    })
}

async fn value_counts(pool: &SqlitePool, sql: &'static str) -> Result<Vec<ValueCount>> {
    Ok(sqlx::query_as::<_, ValueRow>(sql)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| ValueCount {
            value: row.value,
            count: count(row.count),
        })
        .collect())
}

/// Packages per state and per stage, the newest `recent` package transitions, and the
/// downloads per state and failure.
pub(crate) async fn queue_diagnostics(pool: &SqlitePool, recent: u32) -> Result<QueueDiagnostics> {
    let packages_by_state = value_counts(
        pool,
        "SELECT state AS value, COUNT(*) AS count FROM packages GROUP BY state ORDER BY state",
    )
    .await
    .context("count packages per state for diagnostics")?;
    let packages_by_stage = value_counts(
        pool,
        "SELECT postprocess_stage AS value, COUNT(*) AS count FROM packages \
         WHERE postprocess_stage IS NOT NULL GROUP BY postprocess_stage ORDER BY postprocess_stage",
    )
    .await
    .context("count packages per stage for diagnostics")?;
    // A state change is written as a `package_state` event carrying the package, the state
    // before and after and the stage (`set_package_state`); its `current` -- the file being
    // worked on -- is never read. An event whose state did not change is a stage or progress
    // step, not a transition.
    let recent_transitions = sqlx::query_as::<_, PackageTransition>(
        "SELECT json_extract(e.payload_json, '$.package_id') AS package_id, \
         json_extract(e.payload_json, '$.previous') AS previous, \
         json_extract(e.payload_json, '$.state') AS state, \
         json_extract(e.payload_json, '$.stage') AS stage, \
         p.category_id AS category_id, e.occurred_at AS occurred_at \
         FROM events e \
         LEFT JOIN packages p ON p.id = json_extract(e.payload_json, '$.package_id') \
         WHERE e.kind = 'package_state' AND json_valid(e.payload_json) \
         AND json_type(e.payload_json, '$.package_id') = 'text' \
         AND json_type(e.payload_json, '$.state') = 'text' \
         AND json_extract(e.payload_json, '$.previous') IS NOT json_extract(e.payload_json, '$.state') \
         ORDER BY e.occurred_at DESC, e.rowid DESC LIMIT ?",
    )
    .bind(i64::from(recent))
    .fetch_all(pool)
    .await
    .context("read package transitions for diagnostics")?;
    let downloads = sqlx::query_as::<_, DownloadRow>(
        "SELECT state, \
         CASE WHEN json_valid(last_error_json) \
              THEN json_extract(last_error_json, '$.category.kind') END AS error_kind, \
         CASE WHEN json_valid(last_error_json) \
              THEN json_extract(last_error_json, '$.code') END AS error_code, \
         COUNT(*) AS count FROM downloads \
         GROUP BY state, error_kind, error_code ORDER BY state, error_kind, error_code",
    )
    .fetch_all(pool)
    .await
    .context("count downloads for diagnostics")?
    .into_iter()
    .map(|row| DownloadCount {
        state: row.state,
        error_kind: row.error_kind,
        error_code: row.error_code,
        count: count(row.count),
    })
    .collect();
    Ok(QueueDiagnostics {
        packages_by_state,
        packages_by_stage,
        recent_transitions,
        downloads,
    })
}
