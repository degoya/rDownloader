//! The LinkFilter's second word on one link, once its online check ended (RD-1310-01), and the
//! delete a deleting rule asks for, shared with the re-application to the whole list.
//!
//! The verdict `collector.check_no_resolver` exists only after the check, so a rule with the
//! condition "without resolver" cannot decide at intake. When the check of a link is recorded
//! the rules are asked again for that link alone, in the same transaction: a hiding rule hides
//! it, a deleting rule deletes it — without asking, the rule is the consent. Accepting and
//! routing are left to the intake and to "Apply", so a check never moves a link between packages
//! or undoes a hide a person lifted.

use anyhow::Result;
use rd_core::{CandidateId, LinkFilterAction, LinkFilterRuleId};
use sqlx::{FromRow, Sqlite, Transaction};
use url::Url;

use crate::parse_enum;

/// What the rules did with a link after its check.
#[derive(Debug, Default)]
pub(crate) struct CheckFiltered {
    /// The rule that deleted the link; `None` when it stayed.
    pub(crate) deleted_by: Option<LinkFilterRuleId>,
    /// The vault references the deleted row held, for the facade to remove.
    pub(crate) orphaned: Vec<String>,
}

#[derive(FromRow)]
struct CheckedFacts {
    url: String,
    file_name: Option<String>,
    size: Option<i64>,
    state: String,
    error_code: Option<String>,
    hidden_by_filter: Option<String>,
    source: String,
}

/// Whether a row carries the check verdict "no resolver installed for this host": unresolvable,
/// with that code. `collector.check_not_a_file` and every unchecked link are not it.
pub(crate) fn unresolvable_without_resolver(state: &str, error_code: Option<&str>) -> bool {
    state == "unresolvable" && error_code == Some(rd_core::CODE_CHECK_NO_RESOLVER)
}

/// Asks the rules about the link `id` once more, with what its check found.
pub(crate) async fn decide_after_check(
    tx: &mut Transaction<'_, Sqlite>,
    id: CandidateId,
) -> Result<CheckFiltered> {
    let rules = crate::link_filter_store::link_filter_rules(tx).await?;
    let filters = rd_collector::LinkFilters::new(&rules);
    if filters.is_empty() {
        return Ok(CheckFiltered::default());
    }
    let Some(facts) = sqlx::query_as::<_, CheckedFacts>(
        "SELECT c.url, c.file_name, c.size, c.state, c.error_code, c.hidden_by_filter, b.source \
         FROM link_candidates c JOIN collector_batches b ON b.id = c.batch_id WHERE c.id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(CheckFiltered::default());
    };
    let url = Url::parse(&facts.url)?;
    let Some(rule) = filters.decide(&rd_collector::LinkFilterContext {
        source: parse_enum(&facts.source)?,
        url: &url,
        file_name: facts.file_name.as_deref(),
        size: facts.size.and_then(|size| u64::try_from(size).ok()),
        unresolvable_without_resolver: unresolvable_without_resolver(
            &facts.state,
            facts.error_code.as_deref(),
        ),
    }) else {
        return Ok(CheckFiltered::default());
    };
    match rule.action {
        LinkFilterAction::Hide if facts.hidden_by_filter.is_none() => {
            sqlx::query("UPDATE link_candidates SET hidden_by_filter = ? WHERE id = ?")
                .bind(rule.id.to_string())
                .bind(id.to_string())
                .execute(&mut **tx)
                .await?;
            Ok(CheckFiltered::default())
        }
        LinkFilterAction::Delete => {
            tracing::debug!(
                candidate_id = %id,
                rule_id = %rule.id,
                host = url.host_str().unwrap_or_default(),
                "a LinkFilter rule deleted a link after its online check"
            );
            Ok(CheckFiltered {
                deleted_by: Some(rule.id),
                orphaned: delete_rows(tx, &[id.to_string()]).await?,
            })
        }
        LinkFilterAction::Hide | LinkFilterAction::Accept | LinkFilterAction::Route => {
            Ok(CheckFiltered::default())
        }
    }
}

/// Deletes the named LinkGrabber rows a deleting rule matched, a package and a batch they leave
/// empty with them, and answers the vault references the rows held — read first, because the
/// reference is in the row. A row being resolved or already handed over is left alone.
pub(crate) async fn delete_rows(
    tx: &mut Transaction<'_, Sqlite>,
    ids: &[String],
) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = serde_json::to_string(ids)?;
    let references: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT secret_fragment_ref, replay_body_ref FROM link_candidates \
         WHERE id IN (SELECT value FROM json_each(?)) AND state NOT IN ('resolving', 'enqueued')",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM link_candidates \
         WHERE id IN (SELECT value FROM json_each(?)) AND state NOT IN ('resolving', 'enqueued')",
    )
    .bind(&ids)
    .execute(&mut **tx)
    .await?;
    crate::collector_store::delete_empty_batches(tx).await?;
    Ok(references
        .into_iter()
        .flat_map(|(fragment, body)| fragment.into_iter().chain(body))
        .collect())
}
