//! The enqueue lock of a package: claiming its enqueueable links and releasing them.

use anyhow::{Result, bail};
use rd_core::{CandidateId, CollectorPackageId, EventEnvelope, LinkCandidate, LinkCandidateState};
use sqlx::{Connection, SqliteConnection};

use super::collector_event;
use crate::{
    collector_store::{CandidateRow, insert_event},
    enum_string,
    error::{StoreError, StoreErrorKind},
};

/// Atomically locks every enqueueable candidate of a package (`resolving`).
///
/// `only` narrows the claim to the links a person could see: a LinkGrabber filter hides links
/// of a package, and "add to the queue" must not send what it hid. The links left out keep
/// their state and their package, which `finish_package_enqueue` then keeps alive for them.
///
/// A link a LinkFilter rule hid (RD-1240-09) is claimed only when `only` names it: hidden is
/// the server's own filter, so a claim of the whole package leaves it behind like the list's.
pub(crate) async fn claim_package_for_enqueue(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    only: Option<&[CandidateId]>,
) -> Result<Vec<(LinkCandidate, LinkCandidateState)>> {
    let mut tx = connection.begin().await?;
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM link_candidates WHERE package_id = ? AND state IN ('checking', 'resolving')",
    )
    .bind(package_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if busy > 0 {
        bail!(StoreError::busy("package is being processed"));
    }
    // Built from `LinkCandidateState::ENQUEUEABLE` rather than spelled out, so this list and
    // the one the single-link endpoint applies cannot drift apart again. The states are enum
    // variants, not user input, so there is nothing here to inject.
    let states = LinkCandidateState::ENQUEUEABLE
        .iter()
        .map(|state| Ok(format!("'{}'", crate::enum_string(state)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let rows = sqlx::query_as::<_, CandidateRow>(sqlx::AssertSqlSafe(format!(
        "{} WHERE package_id = ? AND state IN ({states}) ORDER BY position, created_at",
        crate::collector_store::CANDIDATE_SELECT
    )))
    .bind(package_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let mut claimed = Vec::with_capacity(rows.len());
    for row in rows {
        let candidate: LinkCandidate = row.try_into()?;
        let named = only.map(|ids| ids.contains(&candidate.id));
        if named == Some(false) || (named.is_none() && candidate.hidden_by_filter.is_some()) {
            continue;
        }
        let previous = candidate.state;
        sqlx::query("UPDATE link_candidates SET state = 'resolving' WHERE id = ?")
            .bind(candidate.id.to_string())
            .execute(&mut *tx)
            .await?;
        claimed.push((candidate, previous));
    }
    if claimed.is_empty() {
        bail!(StoreError::new(
            StoreErrorKind::NoEnqueueableLinks,
            "package has no enqueueable links"
        ));
    }
    tx.commit().await?;
    Ok(claimed)
}

/// Releases the enqueue lock: success marks links `enqueued` and detaches the package,
/// failure restores the previous states.
pub(crate) async fn finish_package_enqueue(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    success: bool,
    restore: &[(CandidateId, LinkCandidateState)],
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    if success {
        // Whatever an enricher stored while this enqueue was running (RD-108-15). The claim
        // handed out a snapshot, so a field written after it reached only the candidate row —
        // and this is the last moment that row can still be matched to what it became.
        let late = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT id, url, enrichment_json FROM link_candidates \
             WHERE package_id = ? AND state = 'resolving'",
        )
        .bind(package_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        for (id, url, enrichment) in late {
            let fields =
                crate::models::parse_enrichment(enrichment.as_deref(), "link_candidates", &id);
            crate::package_store::carry_enrichment_for_source(&mut tx, &url, &fields).await?;
        }
        sqlx::query("UPDATE link_candidates SET state = 'enqueued', package_id = NULL WHERE package_id = ? AND state = 'resolving'")
            .bind(package_id.to_string()).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM collector_packages WHERE id = ? AND NOT EXISTS \
                     (SELECT 1 FROM link_candidates WHERE package_id = collector_packages.id)",
        )
        .bind(package_id.to_string())
        .execute(&mut *tx)
        .await?;
    } else {
        for (id, state) in restore {
            sqlx::query(
                "UPDATE link_candidates SET state = ? WHERE id = ? AND state = 'resolving'",
            )
            .bind(enum_string(*state)?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        }
    }
    crate::collector_store::delete_empty_batches(&mut tx).await?;
    let event =
        collector_event(serde_json::json!({ "package_id": package_id, "enqueued": success }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}
