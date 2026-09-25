//! What the rule self-test found, one row per rule (RD-110-09).
//!
//! The row is a *result*, not a rule: it is keyed by the rule's own identifier and holds no
//! reference to `site_rules`, because the rules the project ships have no row there at all.
//! See `migrations/0081_site_rule_checks.sql`.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// One stored self-test result.
#[derive(Clone, Debug, Deserialize, Eq, FromRow, PartialEq, Serialize)]
pub struct SiteRuleCheck {
    pub rule_id: String,
    /// `ok`, `structural`, `blocked` or `dead`, as `rd_siterules::Verdict` spells it.
    pub verdict: String,
    /// The refusal's stable code; `None` exactly when the rule answered.
    pub code: Option<String>,
    pub links: i64,
    pub pages: i64,
    /// When this run happened, not when the rule was written.
    pub checked_at: String,
}

/// One result about to be written; replaces an earlier one for the same rule.
#[derive(Clone, Debug)]
pub struct NewSiteRuleCheck {
    pub rule_id: String,
    pub verdict: String,
    pub code: Option<String>,
    pub links: i64,
    pub pages: i64,
}

const COLUMNS: &str = "rule_id, verdict, code, links, pages, checked_at";

pub(crate) async fn list_site_rule_checks(pool: &SqlitePool) -> Result<Vec<SiteRuleCheck>> {
    Ok(sqlx::query_as::<_, SiteRuleCheck>(&format!(
        "SELECT {COLUMNS} FROM site_rule_checks ORDER BY rule_id"
    ))
    .fetch_all(pool)
    .await?)
}

/// Writes the results of one self-test run, replacing what an earlier run said about the
/// same rules.
///
/// One transaction and one event for the whole run: a run is the unit a person triggers, and
/// a list that redrew itself once per rule would flicker through a pack of twenty.
pub(crate) async fn record_site_rule_checks(
    connection: &mut SqliteConnection,
    checks: Vec<NewSiteRuleCheck>,
) -> Result<EventEnvelope> {
    let now = Utc::now().to_rfc3339();
    let mut transaction = connection.begin().await?;
    for check in &checks {
        sqlx::query(
            "INSERT INTO site_rule_checks (rule_id, verdict, code, links, pages, checked_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(rule_id) DO UPDATE SET \
               verdict = excluded.verdict, \
               code = excluded.code, \
               links = excluded.links, \
               pages = excluded.pages, \
               checked_at = excluded.checked_at",
        )
        .bind(&check.rule_id)
        .bind(&check.verdict)
        .bind(check.code.as_deref())
        .bind(check.links)
        .bind(check.pages)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
    }
    let event = EventEnvelope::new(
        EventKind::SiteRuleChanged,
        serde_json::json!({ "resource": "site_rule_check", "rules": checks.len() }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}
