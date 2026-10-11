//! LinkFilter rules applied to the LinkGrabber as it is (RD-1240-09): on a person's request,
//! after the rules changed or the online check learned names and sizes.
//!
//! Only LinkGrabber rows are touched — a link already handed to the queue is not a row here any
//! more, and nothing in the downloads changes. A hidden link is never deleted.

use std::collections::BTreeMap;

use anyhow::Result;
use rd_core::{
    BatchId, CandidateId, CategoryId, CollectorPackageId, EventEnvelope, EventKind,
    LinkFilterAction,
};
use sqlx::{Connection, FromRow, SqliteConnection};
use url::Url;

use crate::{parse_enum, parse_id, writer::insert_event};

/// What one application changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinkFilterOutcome {
    /// Links a rule hides now that were shown before.
    pub hidden: u64,
    /// Links shown now that a rule hid before.
    pub shown: u64,
    /// Links a `route` rule moved into another package or gave another category.
    pub routed: u64,
}

#[derive(FromRow)]
struct CandidateFacts {
    id: String,
    url: String,
    file_name: Option<String>,
    size: Option<i64>,
    package_id: String,
    hidden_by_filter: Option<String>,
    source: String,
    batch_id: String,
    package_name: String,
}

/// One route: the package a link goes into (by name, inside its own batch) and the category the
/// package gets.
struct Route {
    candidate: CandidateId,
    from: CollectorPackageId,
    batch: BatchId,
    package_name: Option<String>,
    category: Option<CategoryId>,
}

/// Decides every open LinkGrabber link anew: the hiding rule hides it, an `accept`, a `route` or
/// no rule shows it, and a `route` files it.
pub(crate) async fn apply_link_filters(
    connection: &mut SqliteConnection,
) -> Result<(LinkFilterOutcome, EventEnvelope)> {
    let rules = crate::link_filter_store::link_filter_rules(connection).await?;
    let filters = rd_collector::LinkFilters::new(&rules);
    let mut tx = connection.begin().await?;
    // `resolving` is the enqueue's lock and `enqueued` a record of a hand-over; neither is a
    // link the list still offers.
    let candidates: Vec<CandidateFacts> = sqlx::query_as(
        "SELECT c.id, c.url, c.file_name, c.size, c.package_id, c.hidden_by_filter, b.source, \
         c.batch_id, p.name AS package_name \
         FROM link_candidates c \
         JOIN collector_batches b ON b.id = c.batch_id \
         JOIN collector_packages p ON p.id = c.package_id \
         WHERE c.state NOT IN ('resolving', 'enqueued') \
         ORDER BY p.position, c.position",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut outcome = LinkFilterOutcome::default();
    let mut routes = Vec::new();
    for facts in candidates {
        let url = Url::parse(&facts.url)?;
        let decided = filters.decide(&rd_collector::LinkFilterContext {
            source: parse_enum(&facts.source)?,
            url: &url,
            file_name: facts.file_name.as_deref(),
            size: facts.size.and_then(|size| u64::try_from(size).ok()),
        });
        let hidden_by = decided
            .filter(|rule| rule.action == LinkFilterAction::Hide)
            .map(|rule| rule.id.to_string());
        if hidden_by != facts.hidden_by_filter {
            match (&hidden_by, &facts.hidden_by_filter) {
                (Some(_), None) => outcome.hidden += 1,
                (None, Some(_)) => outcome.shown += 1,
                _ => {}
            }
            sqlx::query("UPDATE link_candidates SET hidden_by_filter = ? WHERE id = ?")
                .bind(&hidden_by)
                .bind(&facts.id)
                .execute(&mut *tx)
                .await?;
        }
        let Some(rule) = decided.filter(|rule| rule.action == LinkFilterAction::Route) else {
            continue;
        };
        let package_name = rule
            .package_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != facts.package_name.as_str())
            .map(str::to_owned);
        if package_name.is_none() && rule.category_id.is_none() {
            continue;
        }
        routes.push(Route {
            candidate: parse_id(&facts.id)?,
            from: parse_id(&facts.package_id)?,
            batch: parse_id(&facts.batch_id)?,
            package_name,
            category: rule.category_id,
        });
    }
    outcome.routed = file_routes(&mut tx, routes).await?;
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({
            "link_filters_applied": true,
            "hidden": outcome.hidden,
            "shown": outcome.shown,
            "routed": outcome.routed,
        }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((outcome, event))
}

/// Moves each routed link into the package of its rule's name in its own batch — the one there,
/// or a new one — and gives that package the rule's category. Answers how many links changed.
async fn file_routes(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    routes: Vec<Route>,
) -> Result<u64> {
    if routes.is_empty() {
        return Ok(0);
    }
    let mut touched: Vec<CollectorPackageId> = Vec::new();
    // A package created here is found again by the next link routed to the same name.
    let mut created: BTreeMap<(BatchId, String), CollectorPackageId> = BTreeMap::new();
    let mut routed = 0_u64;
    for route in routes {
        let target = match &route.package_name {
            None => route.from,
            Some(name) => {
                package_named(tx, &mut created, route.batch, name, route.category).await?
            }
        };
        if let Some(category) = route.category {
            sqlx::query("UPDATE collector_packages SET category_id = ? WHERE id = ?")
                .bind(category.to_string())
                .bind(target.to_string())
                .execute(&mut **tx)
                .await?;
        }
        if target != route.from {
            let next_position: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM link_candidates WHERE package_id = ?",
            )
            .bind(target.to_string())
            .fetch_one(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE link_candidates SET package_id = ?, position = ? WHERE id = ? \
                 AND state NOT IN ('resolving', 'enqueued')",
            )
            .bind(target.to_string())
            .bind(next_position)
            .bind(route.candidate.to_string())
            .execute(&mut **tx)
            .await?;
            touched.push(route.from);
        }
        touched.push(target);
        routed += 1;
    }
    touched.sort_unstable();
    touched.dedup();
    // A link takes its package's category and priority, as a move by hand gives it.
    for package in &touched {
        sqlx::query(
            "UPDATE link_candidates SET \
             category_id = (SELECT category_id FROM collector_packages WHERE id = ?), \
             priority = (SELECT priority FROM collector_packages WHERE id = ?) \
             WHERE package_id = ? AND state NOT IN ('resolving', 'enqueued')",
        )
        .bind(package.to_string())
        .bind(package.to_string())
        .bind(package.to_string())
        .execute(&mut **tx)
        .await?;
    }
    // A mirror group lives inside one package, so both ends of a move are grouped again.
    crate::collector_mirrors::assign(tx, &touched).await?;
    crate::collector_packages::delete_empty_packages(tx).await?;
    Ok(routed)
}

/// The package called `name` in `batch`: one this application filed into already, one that is
/// there, or a new one with the rule's category.
async fn package_named(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    created: &mut BTreeMap<(BatchId, String), CollectorPackageId>,
    batch: BatchId,
    name: &str,
    category: Option<CategoryId>,
) -> Result<CollectorPackageId> {
    let key = (batch, name.to_owned());
    if let Some(id) = created.get(&key).copied() {
        return Ok(id);
    }
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT id FROM collector_packages WHERE batch_id = ? AND name = ? \
         ORDER BY position LIMIT 1",
    )
    .bind(batch.to_string())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await?;
    let id = match existing {
        Some(id) => parse_id(&id)?,
        // The rule stated the name, so it is not auto-named: a regroup leaves it alone.
        None => {
            crate::collector_packages::insert(
                tx,
                batch,
                name,
                false,
                category,
                rd_core::DownloadPriority::default(),
            )
            .await?
        }
    };
    created.insert(key, id);
    Ok(id)
}

/// Shows hidden links again until the rules are applied anew; answers how many were hidden.
pub(crate) async fn show_filtered_candidates(
    connection: &mut SqliteConnection,
    ids: &[CandidateId],
) -> Result<(u64, EventEnvelope)> {
    let mut tx = connection.begin().await?;
    let shown = sqlx::query(
        "UPDATE link_candidates SET hidden_by_filter = NULL \
         WHERE id IN (SELECT value FROM json_each(?)) AND hidden_by_filter IS NOT NULL",
    )
    .bind(serde_json::to_string(
        &ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
    )?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "shown_candidates": shown }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((shown, event))
}
