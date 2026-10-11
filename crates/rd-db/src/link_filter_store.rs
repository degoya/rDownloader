//! LinkFilter rules (RD-1240-09): create, list, change, reorder and delete.
//!
//! Every write emits `collector.changed`: the rules belong to the LinkGrabber, and deleting one
//! shows the links it hid again (`link_candidates.hidden_by_filter` is `ON DELETE SET NULL`).

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind, LinkFilterRule, LinkFilterRuleId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{enum_string, error::StoreError, parse_enum, parse_id, writer::insert_event};

/// The editable fields of a rule; its position is the end of the list on create and kept on
/// update.
#[derive(Clone, Debug)]
pub struct NewLinkFilterRule {
    pub name: String,
    pub enabled: bool,
    pub name_pattern: Option<String>,
    pub name_syntax: rd_core::LinkFilterNameSyntax,
    pub size_min: Option<u64>,
    pub size_max: Option<u64>,
    pub extensions: Vec<String>,
    pub hoster: Option<String>,
    pub source: Option<rd_core::IngressSource>,
    pub action: rd_core::LinkFilterAction,
    pub package_name: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
}

/// The rules in evaluation order. A category that no longer exists reads as none: the column has
/// no foreign key, because a settings import replaces the categories wholesale.
const RULE_SELECT: &str = "SELECT r.id, r.name, r.position, r.enabled, r.name_pattern, \
     r.name_syntax, r.size_min, r.size_max, r.extensions_json, r.hoster, r.source, r.action, \
     r.package_name, \
     CASE WHEN EXISTS (SELECT 1 FROM categories c WHERE c.id = r.category_id) \
          THEN r.category_id END AS category_id \
     FROM link_filter_rules r ORDER BY r.position, r.name";

pub(crate) async fn list_link_filter_rules(pool: &SqlitePool) -> Result<Vec<LinkFilterRule>> {
    sqlx::query_as::<_, RuleRow>(RULE_SELECT)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

/// The same list on the writer's connection, for the intake and the re-application.
pub(crate) async fn link_filter_rules(
    connection: &mut SqliteConnection,
) -> Result<Vec<LinkFilterRule>> {
    sqlx::query_as::<_, RuleRow>(RULE_SELECT)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

pub(crate) async fn create_link_filter_rule(
    connection: &mut SqliteConnection,
    input: NewLinkFilterRule,
) -> Result<(LinkFilterRule, EventEnvelope)> {
    let id = LinkFilterRuleId::new();
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let position: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM link_filter_rules")
            .fetch_one(&mut *tx)
            .await?;
    let columns = StoredColumns::of(&input)?;
    sqlx::query(
        "INSERT INTO link_filter_rules (id, name, position, enabled, name_pattern, name_syntax, \
         size_min, size_max, extensions_json, hoster, source, action, package_name, category_id, \
         created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&input.name)
    .bind(position)
    .bind(input.enabled)
    .bind(&input.name_pattern)
    .bind(&columns.name_syntax)
    .bind(columns.size_min)
    .bind(columns.size_max)
    .bind(&columns.extensions)
    .bind(&input.hoster)
    .bind(&columns.source)
    .bind(&columns.action)
    .bind(&input.package_name)
    .bind(input.category_id.map(|value| value.to_string()))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let event = filter_event(id);
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((rule(id, position, input), event))
}

pub(crate) async fn update_link_filter_rule(
    connection: &mut SqliteConnection,
    id: LinkFilterRuleId,
    input: NewLinkFilterRule,
) -> Result<(LinkFilterRule, EventEnvelope)> {
    let columns = StoredColumns::of(&input)?;
    let mut tx = connection.begin().await?;
    let position: Option<i64> =
        sqlx::query_scalar("SELECT position FROM link_filter_rules WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    let Some(position) = position else {
        anyhow::bail!(StoreError::not_found("link filter rule not found"));
    };
    sqlx::query(
        "UPDATE link_filter_rules SET name = ?, enabled = ?, name_pattern = ?, name_syntax = ?, \
         size_min = ?, size_max = ?, extensions_json = ?, hoster = ?, source = ?, action = ?, \
         package_name = ?, category_id = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(input.enabled)
    .bind(&input.name_pattern)
    .bind(&columns.name_syntax)
    .bind(columns.size_min)
    .bind(columns.size_max)
    .bind(&columns.extensions)
    .bind(&input.hoster)
    .bind(&columns.source)
    .bind(&columns.action)
    .bind(&input.package_name)
    .bind(input.category_id.map(|value| value.to_string()))
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let event = filter_event(id);
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((rule(id, position, input), event))
}

pub(crate) async fn delete_link_filter_rule(
    connection: &mut SqliteConnection,
    id: LinkFilterRuleId,
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM link_filter_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("link filter rule not found"));
    }
    let event = filter_event(id);
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Numbers the listed rules 1..n in the order given, the unlisted ones after them in their
/// current order — the same shape as the candidate order of a package.
pub(crate) async fn reorder_link_filter_rules(
    connection: &mut SqliteConnection,
    ids: &[LinkFilterRuleId],
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let remaining: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT id FROM link_filter_rules ORDER BY position, name")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .filter(|id| !listed.contains(id))
            .collect();
    for (index, id) in listed.iter().chain(remaining.iter()).enumerate() {
        sqlx::query("UPDATE link_filter_rules SET position = ? WHERE id = ?")
            .bind(i64::try_from(index)? + 1)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "resource": "link_filter", "reordered": ids.len() }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

fn filter_event(id: LinkFilterRuleId) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "resource": "link_filter", "id": id }),
    )
}

fn rule(id: LinkFilterRuleId, position: i64, input: NewLinkFilterRule) -> LinkFilterRule {
    LinkFilterRule {
        id,
        name: input.name,
        position,
        enabled: input.enabled,
        name_pattern: input.name_pattern,
        name_syntax: input.name_syntax,
        size_min: input.size_min,
        size_max: input.size_max,
        extensions: input.extensions,
        hoster: input.hoster,
        source: input.source,
        action: input.action,
        package_name: input.package_name,
        category_id: input.category_id,
    }
}

/// The columns of a rule that are stored in another form than they are held.
struct StoredColumns {
    name_syntax: String,
    size_min: Option<i64>,
    size_max: Option<i64>,
    extensions: String,
    source: Option<String>,
    action: String,
}

impl StoredColumns {
    fn of(input: &NewLinkFilterRule) -> Result<Self> {
        Ok(Self {
            name_syntax: enum_string(input.name_syntax)?,
            size_min: input.size_min.map(i64::try_from).transpose()?,
            size_max: input.size_max.map(i64::try_from).transpose()?,
            extensions: serde_json::to_string(&input.extensions)?,
            source: input.source.map(enum_string).transpose()?,
            action: enum_string(input.action)?,
        })
    }
}

#[derive(FromRow)]
struct RuleRow {
    id: String,
    name: String,
    position: i64,
    enabled: bool,
    name_pattern: Option<String>,
    name_syntax: String,
    size_min: Option<i64>,
    size_max: Option<i64>,
    extensions_json: String,
    hoster: Option<String>,
    source: Option<String>,
    action: String,
    package_name: Option<String>,
    category_id: Option<String>,
}

impl TryFrom<RuleRow> for LinkFilterRule {
    type Error = anyhow::Error;

    fn try_from(row: RuleRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            position: row.position,
            enabled: row.enabled,
            name_pattern: row.name_pattern,
            name_syntax: parse_enum(&row.name_syntax)?,
            size_min: row.size_min.map(u64::try_from).transpose()?,
            size_max: row.size_max.map(u64::try_from).transpose()?,
            extensions: serde_json::from_str(&row.extensions_json)?,
            hoster: row.hoster,
            source: row.source.map(|value| parse_enum(&value)).transpose()?,
            action: parse_enum(&row.action)?,
            package_name: row.package_name,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
        })
    }
}
