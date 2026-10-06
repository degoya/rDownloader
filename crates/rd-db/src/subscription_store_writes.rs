//! The subscription writes: create, edit, enable, arm and delete.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, Subscription, SubscriptionId};
use sqlx::{Connection, SqliteConnection};

use super::{COLUMNS, NewSubscription, SubscriptionRow, changed_event, kind_string, mode_string};
use crate::{error::StoreError, writer::insert_event};

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewSubscription,
) -> Result<(Subscription, EventEnvelope)> {
    let now = Utc::now();
    let value = Subscription {
        id: SubscriptionId::new(),
        name: input.name,
        url: input.url,
        kind: input.kind,
        enabled: input.enabled,
        mode: input.mode,
        category_id: input.category_id,
        priority: input.priority,
        interval_seconds: input.interval_seconds,
        filters: input.filters,
        backlog: input.backlog,
        category_map: input.category_map,
        source_categories: input.source_categories,
        primed: false,
        last_run_at: None,
        // No next run: a new subscription is due at once, so its backlog decision is made
        // and shown immediately rather than an interval from now.
        next_run_at: None,
        consecutive_failures: 0,
        last_error: None,
        etag: None,
        last_modified: None,
        has_secret: input.secret_ref.is_some(),
        secret_ref: input.secret_ref,
        every_release: input.every_release,
        view: input.view,
        autoplay: input.autoplay,
        card_ratio: input.card_ratio,
        schedule: input.schedule,
        script_arguments: input.script_arguments,
        indexer_search: input.indexer_search,
        git_release: input.git_release,
        created_at: now,
        updated_at: now,
    };
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, category_id, priority, \
         interval_seconds, filters_json, backlog_json, category_map_json, \
         source_categories_json, every_release, view, autoplay, card_ratio, schedule, \
         script_arguments_json, indexer_search_json, git_release_json, primed, \
         consecutive_failures, secret_ref, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(value.url.as_str())
    .bind(kind_string(value.kind))
    .bind(i64::from(value.enabled))
    .bind(mode_string(value.mode))
    .bind(value.category_id.map(|id| id.to_string()))
    .bind(i64::from(value.priority.as_i32()))
    .bind(i64::from(value.interval_seconds))
    .bind(serde_json::to_string(&value.filters)?)
    .bind(serde_json::to_string(&value.backlog)?)
    .bind(serde_json::to_string(&value.category_map)?)
    .bind(serde_json::to_string(&value.source_categories)?)
    .bind(i64::from(value.every_release))
    .bind(value.view.as_str())
    .bind(i64::from(value.autoplay))
    .bind(value.card_ratio.as_str())
    .bind(value.schedule.as_deref())
    .bind(serde_json::to_string(&value.script_arguments)?)
    .bind(serde_json::to_string(&value.indexer_search)?)
    .bind(serde_json::to_string(&value.git_release)?)
    .bind(value.secret_ref.as_deref())
    .bind(value.created_at)
    .bind(value.updated_at)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Applies an edit. `secret_ref` is only replaced when `Some`, so an unchanged API key is
/// not silently dropped by a form that does not resend it.
///
/// A changed schedule clears the next run (RD-130-19): the time the old expression computed
/// means nothing under the new one, and the poller arms a cleared scheduled row with the new
/// expression's next occurrence rather than running it.
///
/// A changed address or changed git-release options clear the stored validators
/// (RD-190-13): an unchanged release list answers `304`, and the assets the new options
/// select would otherwise be looked at only once the repository publishes something else.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
    input: NewSubscription,
) -> Result<(Subscription, Option<String>, EventEnvelope)> {
    let existing = sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(&mut *connection)
    .await?
    .context(StoreError::not_found("subscription not found"))?;
    let existing: Subscription = existing.try_into()?;
    // The replaced reference is handed back so the caller can remove it from the vault; a
    // reference nothing points at is an orphan that would never be cleaned up.
    let orphan = match (&input.secret_ref, &existing.secret_ref) {
        (Some(new), Some(old)) if new != old => Some(old.clone()),
        _ => None,
    };
    let secret_ref = input.secret_ref.or(existing.secret_ref);
    let git_release = serde_json::to_string(&input.git_release)?;
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE subscriptions SET name = ?, url = ?, kind = ?, enabled = ?, mode = ?, \
         category_id = ?, priority = ?, interval_seconds = ?, filters_json = ?, \
         backlog_json = ?, category_map_json = ?, source_categories_json = ?, \
         every_release = ?, view = ?, autoplay = ?, card_ratio = ?, secret_ref = ?, \
         script_arguments_json = ?, indexer_search_json = ?, git_release_json = ?, \
         etag = CASE WHEN url IS ? AND git_release_json IS ? THEN etag ELSE NULL END, \
         last_modified = CASE WHEN url IS ? AND git_release_json IS ? THEN last_modified \
           ELSE NULL END, \
         next_run_at = CASE WHEN schedule IS ? THEN next_run_at ELSE NULL END, schedule = ?, \
         updated_at = ? \
         WHERE id = ?",
    )
    .bind(&input.name)
    .bind(input.url.as_str())
    .bind(kind_string(input.kind))
    .bind(i64::from(input.enabled))
    .bind(mode_string(input.mode))
    .bind(input.category_id.map(|id| id.to_string()))
    .bind(i64::from(input.priority.as_i32()))
    .bind(i64::from(input.interval_seconds))
    .bind(serde_json::to_string(&input.filters)?)
    .bind(serde_json::to_string(&input.backlog)?)
    .bind(serde_json::to_string(&input.category_map)?)
    .bind(serde_json::to_string(&input.source_categories)?)
    .bind(i64::from(input.every_release))
    .bind(input.view.as_str())
    .bind(i64::from(input.autoplay))
    .bind(input.card_ratio.as_str())
    .bind(secret_ref.as_deref())
    .bind(serde_json::to_string(&input.script_arguments)?)
    .bind(serde_json::to_string(&input.indexer_search)?)
    .bind(git_release.as_str())
    .bind(input.url.as_str())
    .bind(git_release.as_str())
    .bind(input.url.as_str())
    .bind(git_release.as_str())
    .bind(input.schedule.as_deref())
    .bind(input.schedule.as_deref())
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let updated = sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((updated, orphan, event))
}

pub(crate) async fn set_enabled(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
    enabled: bool,
) -> Result<(Subscription, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let result = sqlx::query("UPDATE subscriptions SET enabled = ?, updated_at = ? WHERE id = ?")
        .bind(i64::from(enabled))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("subscription not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let updated = sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((updated, event))
}

/// Gives a scheduled subscription that has never been timed its first due time (RD-130-19).
///
/// Only a row whose next run is still empty is touched, so an arm racing a finished run or
/// an edit changes nothing that either of them wrote. No event: nothing a client shows moved
/// except the time, and the next finished run announces itself anyway.
pub(crate) async fn arm(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
    next_run_at: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE subscriptions SET next_run_at = ? WHERE id = ? AND next_run_at IS NULL",
    )
    .bind(next_run_at)
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a subscription and everything it archived, returning its secret reference so the
/// caller can drop the vault entry too.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
) -> Result<(Option<String>, EventEnvelope)> {
    let secret_ref: Option<Option<String>> =
        sqlx::query_scalar("SELECT secret_ref FROM subscriptions WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
    let secret_ref = secret_ref.context(StoreError::not_found("subscription not found"))?;
    let event = changed_event();
    let mut tx = connection.begin().await?;
    // The child tables declare ON DELETE CASCADE, but the deletes are explicit because the
    // foreign-key pragma is a connection setting and a future connection that forgot it
    // would otherwise leave orphans behind rather than fail.
    sqlx::query("DELETE FROM subscription_items WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM subscription_runs WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM subscriptions WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((secret_ref, event))
}
