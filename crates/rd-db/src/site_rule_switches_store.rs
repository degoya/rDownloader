//! Which site rules and which rule groups are switched off (RD-110-08).
//!
//! The switch of a rule is `site_rules.enabled` and stays there. This table is for the switch
//! that has nowhere else to live: a group, which is a property of rule bodies rather than a
//! record of its own. See `migrations/0082_site_rule_switches.sql`. Until RD-130-07 it also
//! held the switches of the rules compiled into the binary, which had no row anywhere;
//! migration `0095` removed those rows along with that pack, and the service reads the
//! `group` scope alone.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// The `scope` of a switch about one rule of the former compiled-in pack. Nothing writes it
/// since RD-130-07; the schema's `CHECK` still admits it, and a migration that is applied may
/// not be edited.
pub const SCOPE_RULE: &str = "rule";
/// The `scope` of a switch about a whole group.
pub const SCOPE_GROUP: &str = "group";

/// One stored decision. Absence of a row means the rule or group is on.
#[derive(Clone, Debug, Deserialize, Eq, FromRow, PartialEq, Serialize)]
pub struct SiteRuleSwitch {
    /// [`SCOPE_RULE`] or [`SCOPE_GROUP`].
    pub scope: String,
    /// The rule's id, or the group's name.
    pub key: String,
    pub enabled: bool,
}

const COLUMNS: &str = "scope, \"key\", enabled";

pub(crate) async fn list_site_rule_switches(pool: &SqlitePool) -> Result<Vec<SiteRuleSwitch>> {
    Ok(sqlx::query_as::<_, SiteRuleSwitch>(&format!(
        "SELECT {COLUMNS} FROM site_rule_switches ORDER BY scope, \"key\""
    ))
    .fetch_all(pool)
    .await?)
}

/// Records one decision, replacing whatever the last one said about the same subject.
pub(crate) async fn set_site_rule_switch(
    connection: &mut SqliteConnection,
    scope: &str,
    key: &str,
    enabled: bool,
) -> Result<EventEnvelope> {
    let now = Utc::now().to_rfc3339();
    let mut transaction = sqlx::Connection::begin(connection).await?;
    sqlx::query(
        "INSERT INTO site_rule_switches (scope, \"key\", enabled, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(scope, \"key\") DO UPDATE SET \
           enabled = excluded.enabled, \
           updated_at = excluded.updated_at",
    )
    .bind(scope)
    .bind(key)
    .bind(enabled)
    .bind(&now)
    .execute(&mut *transaction)
    .await?;
    let event = EventEnvelope::new(
        EventKind::SiteRuleChanged,
        serde_json::json!({ "resource": "site_rule_switch", "scope": scope, "key": key }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}
