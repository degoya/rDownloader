//! Category routing rules: create, list, edit, delete and the routing configuration.

use anyhow::Result;
use chrono::Utc;
use rd_core::{CategoryId, CategoryRule, CategoryRuleId, EventEnvelope, EventKind};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::{NewCategoryRule, config_event};
use crate::{enum_string, error::StoreError, parse_enum, parse_id, writer::insert_event};

const RULE_COLUMNS: &str = "id, name, priority, source, domain, protocol, extension, mime_type, name_regex, category_id, enabled";

pub(crate) async fn create_category_rule(
    connection: &mut SqliteConnection,
    input: NewCategoryRule,
) -> Result<(CategoryRule, EventEnvelope)> {
    let value = CategoryRule {
        id: CategoryRuleId::new(),
        name: input.name,
        priority: input.priority,
        source: input.source,
        domain: input.domain,
        protocol: input.protocol,
        extension: input.extension,
        mime_type: input.mime_type,
        name_regex: input.name_regex,
        category_id: input.category_id,
        enabled: input.enabled,
    };
    let now = Utc::now();
    let event = config_event(EventKind::CategoryChanged, "category_rule", value.id);
    let source = value.source.map(enum_string).transpose()?;
    let mut tx = connection.begin().await?;
    sqlx::query("INSERT INTO category_rules (id, name, priority, source, domain, protocol, extension, mime_type, name_regex, category_id, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(value.id.to_string())
        .bind(&value.name)
        .bind(value.priority)
        .bind(source)
        .bind(&value.domain)
        .bind(&value.protocol)
        .bind(&value.extension)
        .bind(&value.mime_type)
        .bind(&value.name_regex)
        .bind(value.category_id.to_string())
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn list_category_rules(pool: &SqlitePool) -> Result<Vec<CategoryRule>> {
    sqlx::query_as::<_, RuleRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM category_rules ORDER BY priority, name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn routing_config(
    connection: &mut SqliteConnection,
) -> Result<(Vec<CategoryRule>, Option<CategoryId>)> {
    let rules = sqlx::query_as::<_, RuleRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM category_rules ORDER BY priority, name"
    )))
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect::<Result<Vec<_>>>()?;
    let default = sqlx::query_scalar::<_, String>(
        "SELECT id FROM categories WHERE is_default = 1 ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_optional(&mut *connection)
    .await?
    .as_deref()
    .map(parse_id)
    .transpose()?;
    Ok((rules, default))
}

pub(crate) async fn update_category_rule(
    connection: &mut SqliteConnection,
    id: CategoryRuleId,
    input: NewCategoryRule,
) -> Result<(CategoryRule, EventEnvelope)> {
    let value = CategoryRule {
        id,
        name: input.name,
        priority: input.priority,
        source: input.source,
        domain: input.domain,
        protocol: input.protocol,
        extension: input.extension,
        mime_type: input.mime_type,
        name_regex: input.name_regex,
        category_id: input.category_id,
        enabled: input.enabled,
    };
    let event = config_event(EventKind::CategoryChanged, "category_rule", id);
    let source = value.source.map(enum_string).transpose()?;
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE category_rules SET name = ?, priority = ?, source = ?, domain = ?, protocol = ?, \
         extension = ?, mime_type = ?, name_regex = ?, category_id = ?, enabled = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(&value.name)
    .bind(value.priority)
    .bind(source)
    .bind(&value.domain)
    .bind(&value.protocol)
    .bind(&value.extension)
    .bind(&value.mime_type)
    .bind(&value.name_regex)
    .bind(value.category_id.to_string())
    .bind(value.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category rule not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_category_rule(
    connection: &mut SqliteConnection,
    id: CategoryRuleId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::CategoryChanged, "category_rule", id);
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM category_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("category rule not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

#[derive(FromRow)]
struct RuleRow {
    id: String,
    name: String,
    priority: i32,
    source: Option<String>,
    domain: Option<String>,
    protocol: Option<String>,
    extension: Option<String>,
    mime_type: Option<String>,
    name_regex: Option<String>,
    category_id: String,
    enabled: bool,
}
impl TryFrom<RuleRow> for CategoryRule {
    type Error = anyhow::Error;
    fn try_from(row: RuleRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            priority: row.priority,
            source: row.source.map(|value| parse_enum(&value)).transpose()?,
            domain: row.domain,
            protocol: row.protocol,
            extension: row.extension,
            mime_type: row.mime_type,
            name_regex: row.name_regex,
            category_id: parse_id(&row.category_id)?,
            enabled: row.enabled,
        })
    }
}
