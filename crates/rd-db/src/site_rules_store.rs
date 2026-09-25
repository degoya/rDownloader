//! User-written site rules (RD-110-04).
//!
//! The body is stored as the JSON the system boundary validated and handed back unchanged;
//! this module knows a rule's identity, its group and whether it is switched on, and nothing
//! about what is inside. See `migrations/0076_site_rules.sql` for why.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// One stored user rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct UserSiteRule {
    /// The rule's own identifier, as its body carries it.
    pub id: String,
    pub name: String,
    pub group: String,
    pub enabled: bool,
    /// The rule as `rd_siterules::Rule` serialises it.
    pub rule: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

/// A rule about to be written; replaces an earlier one of the same id.
#[derive(Clone, Debug)]
pub struct NewUserSiteRule {
    pub id: String,
    pub name: String,
    pub group: String,
    pub enabled: bool,
    pub rule: serde_json::Value,
}

#[derive(FromRow)]
struct Row {
    id: String,
    name: String,
    rule_group: String,
    enabled: bool,
    rule_json: String,
    created_at: String,
    updated_at: String,
}

impl TryFrom<Row> for UserSiteRule {
    type Error = anyhow::Error;

    fn try_from(row: Row) -> Result<Self> {
        let rule = serde_json::from_str(&row.rule_json)
            .with_context(|| format!("site rule {} holds invalid JSON", row.id))?;
        Ok(Self {
            id: row.id,
            name: row.name,
            group: row.rule_group,
            enabled: row.enabled,
            rule,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

const COLUMNS: &str = "id, name, rule_group, enabled, rule_json, created_at, updated_at";

pub(crate) async fn list_site_rules(pool: &SqlitePool) -> Result<Vec<UserSiteRule>> {
    let rows = sqlx::query_as::<_, Row>(&format!("SELECT {COLUMNS} FROM site_rules ORDER BY id"))
        .fetch_all(pool)
        .await?;
    rows.into_iter().map(UserSiteRule::try_from).collect()
}

/// Writes a rule, replacing the body of an earlier one with the same id and keeping when it
/// was first created.
pub(crate) async fn upsert_site_rule(
    connection: &mut SqliteConnection,
    input: NewUserSiteRule,
) -> Result<(UserSiteRule, EventEnvelope)> {
    let now = Utc::now().to_rfc3339();
    let rule_json = serde_json::to_string(&input.rule)?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO site_rules \
           (id, name, rule_group, enabled, rule_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           name = excluded.name, \
           rule_group = excluded.rule_group, \
           enabled = excluded.enabled, \
           rule_json = excluded.rule_json, \
           updated_at = excluded.updated_at",
    )
    .bind(&input.id)
    .bind(&input.name)
    .bind(&input.group)
    .bind(input.enabled)
    .bind(&rule_json)
    .bind(&now)
    .bind(&now)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, Row>(&format!("SELECT {COLUMNS} FROM site_rules WHERE id = ?"))
        .bind(&input.id)
        .fetch_one(&mut *transaction)
        .await?;
    let event = rule_event(&input.id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((UserSiteRule::try_from(row)?, event))
}

/// Removes a rule; the event is `None` when there was nothing to remove, so a client is not
/// told to refetch a list that did not change.
pub(crate) async fn delete_site_rule(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<(bool, Option<EventEnvelope>)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM site_rules WHERE id = ?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    if result.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok((false, None));
    }
    let event = rule_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((true, Some(event)))
}

/// What a write announces: which rule, never its body. The body is configuration the
/// operator wrote and is read back through the list; the event only says the list changed.
fn rule_event(id: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SiteRuleChanged,
        serde_json::json!({ "resource": "site_rule", "rule_id": id }),
    )
}
