//! Which installed version of a plugin runs, which is under test, and how updates arrive
//! (RD-140-02).
//!
//! The rows are pointers the operator set; which versions exist is the plugin host's business.
//! The host reads them once at start, so a change here takes effect at the next start, the way
//! installing and switching a plugin off do. The rules that turn a pointer into the version
//! that runs live in `rd_plugin_host::default_version`, not here.

use anyhow::{Result, bail};
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// How updates for one plugin arrive.
pub const UPDATE_POLICIES: [&str; 2] = ["manual", "automatic"];

/// One plugin's version choice.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginVersionChoice {
    pub plugin_id: String,
    /// The version new work runs on from the next start; `None` means the newest.
    pub active_version: Option<String>,
    /// What a rollback returns to.
    pub previous_version: Option<String>,
    /// The version under test, reached only by a download pinned to it.
    pub staged_version: Option<String>,
    /// `manual` or `automatic`.
    pub update_policy: String,
    pub updated_at: String,
}

/// A choice about to be stored, replacing the plugin's row as a whole.
#[derive(Clone, Debug)]
pub struct NewPluginVersionChoice {
    pub plugin_id: String,
    pub active_version: Option<String>,
    pub previous_version: Option<String>,
    pub staged_version: Option<String>,
    pub update_policy: String,
}

pub(crate) async fn list_plugin_version_choices(
    pool: &SqlitePool,
) -> Result<Vec<PluginVersionChoice>> {
    let rows = sqlx::query_as::<_, PluginVersionChoice>(
        "SELECT plugin_id, active_version, previous_version, staged_version, update_policy, \
         updated_at FROM plugin_version_choices ORDER BY plugin_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub(crate) async fn plugin_version_choice(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<Option<PluginVersionChoice>> {
    let row = sqlx::query_as::<_, PluginVersionChoice>(
        "SELECT plugin_id, active_version, previous_version, staged_version, update_policy, \
         updated_at FROM plugin_version_choices WHERE plugin_id = ?",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Stores one plugin's choice as a whole.
///
/// A whole-row replace rather than per-column updates: activating, staging and rolling back
/// each move two pointers at once (a rollback swaps active and previous), and one write is what
/// makes the switch atomic for everything that starts after it.
pub(crate) async fn save_plugin_version_choice(
    connection: &mut SqliteConnection,
    input: NewPluginVersionChoice,
) -> Result<(PluginVersionChoice, EventEnvelope)> {
    if !UPDATE_POLICIES.contains(&input.update_policy.as_str()) {
        bail!(crate::StoreError::wrong_state(
            "unknown plugin update policy"
        ));
    }
    if input.staged_version.is_some() && input.staged_version == input.active_version {
        bail!(crate::StoreError::wrong_state(
            "a plugin version cannot be active and under test at once"
        ));
    }
    let value = PluginVersionChoice {
        plugin_id: input.plugin_id,
        active_version: input.active_version,
        previous_version: input.previous_version,
        staged_version: input.staged_version,
        update_policy: input.update_policy,
        updated_at: Utc::now().to_rfc3339(),
    };
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO plugin_version_choices \
           (plugin_id, active_version, previous_version, staged_version, update_policy, \
            updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(plugin_id) DO UPDATE SET \
           active_version = excluded.active_version, \
           previous_version = excluded.previous_version, \
           staged_version = excluded.staged_version, \
           update_policy = excluded.update_policy, \
           updated_at = excluded.updated_at",
    )
    .bind(&value.plugin_id)
    .bind(&value.active_version)
    .bind(&value.previous_version)
    .bind(&value.staged_version)
    .bind(&value.update_policy)
    .bind(&value.updated_at)
    .execute(&mut *transaction)
    .await?;
    // `PluginChanged`, administration-scoped like the routes that write this table; it names
    // the plugin and nothing else, and the inventory is re-read from it.
    let event = EventEnvelope::new(
        EventKind::PluginChanged,
        serde_json::json!({
            "resource": "plugin",
            "plugin_id": value.plugin_id,
            "action": "version_choice",
        }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((value, event))
}
