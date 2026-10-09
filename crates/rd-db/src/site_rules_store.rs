//! User-written site rules (RD-110-04).
//!
//! The body is stored as the JSON the system boundary validated and handed back unchanged;
//! this module knows a rule's identity, its group and whether it is switched on, and nothing
//! about what is inside. See `migrations/0076_site_rules.sql` for why. Since RD-1200-05 it
//! also knows where a rule came from (`migrations/0132_site_rule_origin.sql`) and the highest
//! sequence of a signed rule file accepted per signer.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// How a rule reached this installation (RD-1200-05).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SiteRuleOriginKind {
    /// The signed release file; the origin names the signer and the file's sequence.
    Signed,
    /// An unsigned file or a pasted export.
    Import,
    /// Written or changed in the settings page.
    Editor,
    /// Written through an MCP tool.
    Mcp,
    /// Stored by a build before RD-1200-05, or a word this build does not know.
    Unknown,
}

impl SiteRuleOriginKind {
    /// The word the `origin` column holds.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signed => "signed",
            Self::Import => "import",
            Self::Editor => "editor",
            Self::Mcp => "mcp",
            Self::Unknown => "unknown",
        }
    }

    /// Reads the column; a word a later build wrote is `Unknown` rather than a guess.
    #[must_use]
    pub fn parse(word: &str) -> Self {
        match word {
            "signed" => Self::Signed,
            "import" => Self::Import,
            "editor" => Self::Editor,
            "mcp" => Self::Mcp,
            _ => Self::Unknown,
        }
    }
}

/// Where a stored rule came from: the path that wrote its current body (RD-1200-05).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SiteRuleOrigin {
    pub kind: SiteRuleOriginKind,
    /// The key whose signature held, for [`SiteRuleOriginKind::Signed`] only.
    pub signer: Option<String>,
    /// The signed file's sequence, for [`SiteRuleOriginKind::Signed`] only.
    pub sequence: Option<u64>,
}

impl SiteRuleOrigin {
    /// A rule from a signed file.
    #[must_use]
    pub fn signed(signer: &str, sequence: u64) -> Self {
        Self {
            kind: SiteRuleOriginKind::Signed,
            signer: Some(signer.to_owned()),
            sequence: Some(sequence),
        }
    }

    /// A rule from any path that carries no signature.
    #[must_use]
    pub fn unsigned(kind: SiteRuleOriginKind) -> Self {
        Self {
            kind,
            signer: None,
            sequence: None,
        }
    }
}

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
    /// Where the rule's current body came from (RD-1200-05).
    pub origin: SiteRuleOrigin,
}

/// A rule about to be written; replaces an earlier one of the same id.
#[derive(Clone, Debug)]
pub struct NewUserSiteRule {
    pub id: String,
    pub name: String,
    pub group: String,
    pub enabled: bool,
    pub rule: serde_json::Value,
    /// Where this body came from; a switch passes the stored origin on unchanged.
    pub origin: SiteRuleOrigin,
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
    origin: String,
    origin_signer: Option<String>,
    origin_sequence: Option<i64>,
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
            origin: SiteRuleOrigin {
                kind: SiteRuleOriginKind::parse(&row.origin),
                signer: row.origin_signer,
                sequence: row
                    .origin_sequence
                    .and_then(|sequence| u64::try_from(sequence).ok()),
            },
        })
    }
}

const COLUMNS: &str = "id, name, rule_group, enabled, rule_json, created_at, updated_at, \
                       origin, origin_signer, origin_sequence";

pub(crate) async fn list_site_rules(pool: &SqlitePool) -> Result<Vec<UserSiteRule>> {
    let rows = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM site_rules ORDER BY id"
    )))
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
    let sequence = input
        .origin
        .sequence
        .map(i64::try_from)
        .transpose()
        .context("a site rule's sequence is out of range")?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO site_rules \
           (id, name, rule_group, enabled, rule_json, created_at, updated_at, \
            origin, origin_signer, origin_sequence) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           name = excluded.name, \
           rule_group = excluded.rule_group, \
           enabled = excluded.enabled, \
           rule_json = excluded.rule_json, \
           updated_at = excluded.updated_at, \
           origin = excluded.origin, \
           origin_signer = excluded.origin_signer, \
           origin_sequence = excluded.origin_sequence",
    )
    .bind(&input.id)
    .bind(&input.name)
    .bind(&input.group)
    .bind(input.enabled)
    .bind(&rule_json)
    .bind(&now)
    .bind(&now)
    .bind(input.origin.kind.as_str())
    .bind(&input.origin.signer)
    .bind(sequence)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM site_rules WHERE id = ?"
    )))
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

/// Records that a signed rule file of `sequence` from `signer` was accepted and answers the
/// highest sequence accepted from that signer *before* this call (RD-1200-05).
///
/// Read and write are one transaction on the serialised writer, so two imports at once cannot
/// both measure against the same old mark. The stored mark only ever rises (`MAX`): an older
/// file leaves it alone, and the caller refuses that file on the answer.
pub(crate) async fn record_site_rule_pack(
    connection: &mut SqliteConnection,
    signer: &str,
    sequence: u64,
) -> Result<Option<u64>> {
    let sequence = i64::try_from(sequence).context("a rule file's sequence is out of range")?;
    let mut transaction = connection.begin().await?;
    let known: Option<i64> =
        sqlx::query_scalar("SELECT sequence FROM site_rule_pack_sequences WHERE signer = ?")
            .bind(signer)
            .fetch_optional(&mut *transaction)
            .await?;
    sqlx::query(
        "INSERT INTO site_rule_pack_sequences (signer, sequence, accepted_at) \
         VALUES (?, ?, ?) \
         ON CONFLICT(signer) DO UPDATE SET \
           sequence = MAX(sequence, excluded.sequence), \
           accepted_at = CASE WHEN excluded.sequence > sequence \
                              THEN excluded.accepted_at ELSE accepted_at END",
    )
    .bind(signer)
    .bind(sequence)
    .bind(Utc::now().to_rfc3339())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(known.and_then(|known| u64::try_from(known).ok()))
}

/// What a write announces: which rule, never its body. The body is configuration the
/// operator wrote and is read back through the list; the event only says the list changed.
fn rule_event(id: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SiteRuleChanged,
        serde_json::json!({ "resource": "site_rule", "rule_id": id }),
    )
}
