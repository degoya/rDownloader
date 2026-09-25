//! Persistence of subscriptions, their items and their poll history (RD-080-07).
//!
//! The interesting method here is [`record_items`]. It relies on the UNIQUE index over
//! `(subscription_id, item_key)` rather than on a read-then-write: two polls that overlap,
//! or a poll interrupted halfway and repeated after a restart, must not produce two rows
//! for one item, and `INSERT … ON CONFLICT` against the UNIQUE index is the only version of
//! that which is true without holding a lock across the network call.
//!
//! A repeat poll refreshes an item nobody has decided on yet — its address and the media type
//! the feed declares — because those are the feed's to correct and a stale one is what an
//! archived item is stuck with otherwise. Anything already queued, skipped or dismissed is
//! left exactly as it is: re-listing an entry must never undo a decision.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    BacklogPolicy, CategoryId, DownloadPriority, EventEnvelope, EventKind, FilterReason,
    Subscription, SubscriptionCardRatio, SubscriptionFilters, SubscriptionHistoryClearResponse,
    SubscriptionId, SubscriptionItem, SubscriptionItemCounts, SubscriptionItemId,
    SubscriptionItemPage, SubscriptionItemState, SubscriptionKind, SubscriptionMode,
    SubscriptionReviewCount, SubscriptionReviewSummary, SubscriptionRun, SubscriptionRunId,
    SubscriptionView,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, writer::insert_event};

/// Editable fields of a subscription; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewSubscription {
    pub name: String,
    pub url: Url,
    pub kind: SubscriptionKind,
    pub enabled: bool,
    pub mode: SubscriptionMode,
    pub category_id: Option<CategoryId>,
    pub priority: DownloadPriority,
    pub interval_seconds: u32,
    pub filters: SubscriptionFilters,
    pub backlog: BacklogPolicy,
    pub category_map: Vec<rd_core::CategoryMapping>,
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21).
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits, and whether the cards turn on their own
    /// (RD-120-37).
    pub view: SubscriptionView,
    pub autoplay: bool,
    /// The shape of a card's image area in the card view (RD-120-42).
    pub card_ratio: SubscriptionCardRatio,
    /// A cron expression that replaces the interval (RD-130-19), already validated.
    pub schedule: Option<String>,
    pub secret_ref: Option<String>,
}

/// One item as the poller decided it, ready to be archived.
#[derive(Clone, Debug)]
pub struct NewSubscriptionItem {
    pub item_key: String,
    pub title: String,
    pub url: Url,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u32>,
    pub state: SubscriptionItemState,
    pub reason: Option<FilterReason>,
    pub source_category: Option<String>,
    /// The media type the source declared for `url`, when it declared one.
    pub media_type: Option<String>,
    /// What the source said about the release, already filtered (RD-101-17).
    pub attributes: BTreeMap<String, String>,
    /// The archive password the source announced. Stored apart from `attributes` because it
    /// is a secret, and read back only by the intake that hands it to the package.
    pub password: Option<String>,
}

/// Outcome of one poll, written when it finishes.
#[derive(Clone, Debug)]
pub struct PollResult {
    pub found: u32,
    pub accepted: u32,
    pub skipped: u32,
    pub error: Option<String>,
    pub next_run_at: DateTime<Utc>,
    pub consecutive_failures: u32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

const COLUMNS: &str = "id, name, url, kind, enabled, mode, category_id, priority, \
     interval_seconds, filters_json, backlog_json, category_map_json, \
     source_categories_json, every_release, view, autoplay, card_ratio, schedule, primed, \
     last_run_at, next_run_at, consecutive_failures, last_error, etag, last_modified, \
     secret_ref, created_at, updated_at";

const ITEM_COLUMNS: &str = "id, subscription_id, item_key, title, url, published_at, \
     duration_seconds, state, reason, source_category, media_type, attributes_json, \
     password, discovered_at";

/// Serializes the attribute map, or `None` when there is nothing to say.
///
/// `None` rather than `"{}"` matters: the upsert coalesces onto the stored value, so an
/// empty object would blank the details of a row a later poll answered without them.
fn attributes_json(attributes: &BTreeMap<String, String>) -> Option<String> {
    if attributes.is_empty() {
        return None;
    }
    serde_json::to_string(attributes).ok()
}

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SubscriptionChanged,
        serde_json::json!({ "resource": "subscription" }),
    )
}

/// The same event, carrying what a finished poll has to say (RD-106-09).
///
/// `changed_event` is anonymous on purpose: every write in this module emits it and a client
/// simply re-reads what it is showing. A finished poll is the one case where that is not
/// enough. The "check now" action answers before the poll runs, so the only thing that can
/// tell the interface *which* subscription stopped being busy — and whether the check found
/// anything — is the event itself. Without these fields a client could not tell a completed
/// poll from an unrelated edit of another subscription.
///
/// Deliberately not named `accepted_items`: that key is what `rd-api`'s automation context
/// looks for, and reviving a trigger is not this event's business.
fn run_finished_event(subscription_id: SubscriptionId, result: &PollResult) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SubscriptionChanged,
        serde_json::json!({
            "resource": "subscription",
            "poll": "finished",
            "subscription_id": subscription_id.to_string(),
            "found": result.found,
            "accepted": result.accepted,
            "skipped": result.skipped,
            // Already redacted by the caller; it is what the row shows as well.
            "error": result.error,
        }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions ORDER BY name, created_at"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(pool: &SqlitePool, id: SubscriptionId) -> Result<Option<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// Subscriptions whose next run has come, oldest due first.
///
/// A row with no `next_run_at` has never run and is due immediately, which is what makes a
/// freshly created subscription poll without waiting a full interval.
pub(crate) async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions \
         WHERE enabled = 1 AND (next_run_at IS NULL OR next_run_at <= ?) \
         ORDER BY next_run_at IS NOT NULL, next_run_at"
    ))
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn items(
    pool: &SqlitePool,
    id: SubscriptionId,
    limit: i64,
) -> Result<Vec<SubscriptionItem>> {
    sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM subscription_items WHERE subscription_id = ? \
         ORDER BY discovered_at DESC, id DESC LIMIT ?"
    ))
    .bind(id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// One state-filtered archive page plus totals calculated across the complete subscription.
pub(crate) async fn item_page(
    pool: &SqlitePool,
    id: SubscriptionId,
    state: Option<SubscriptionItemState>,
    limit: i64,
    offset: i64,
) -> Result<SubscriptionItemPage> {
    let state = state.map(item_state_string);
    let rows = sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM subscription_items \
         WHERE subscription_id = ? AND (? IS NULL OR state = ?) \
         ORDER BY discovered_at DESC, id DESC LIMIT ? OFFSET ?"
    ))
    .bind(id.to_string())
    .bind(state)
    .bind(state)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM subscription_items \
         WHERE subscription_id = ? AND (? IS NULL OR state = ?)",
    )
    .bind(id.to_string())
    .bind(state)
    .bind(state)
    .fetch_one(pool)
    .await?;
    let counts = item_counts(pool, id).await?;
    let run_total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM subscription_runs WHERE subscription_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(SubscriptionItemPage {
        items: rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?,
        total: u64::try_from(total).unwrap_or_default(),
        counts,
        run_total: u64::try_from(run_total).unwrap_or_default(),
    })
}

async fn item_counts(pool: &SqlitePool, id: SubscriptionId) -> Result<SubscriptionItemCounts> {
    let row = sqlx::query_as::<_, ItemCountRow>(
        "SELECT \
           COALESCE(SUM(CASE WHEN state = 'pending' THEN 1 ELSE 0 END), 0) AS pending, \
           COALESCE(SUM(CASE WHEN state = 'queued' THEN 1 ELSE 0 END), 0) AS queued, \
           COALESCE(SUM(CASE WHEN state = 'skipped' THEN 1 ELSE 0 END), 0) AS skipped, \
           COALESCE(SUM(CASE WHEN state = 'dismissed' THEN 1 ELSE 0 END), 0) AS dismissed \
         FROM subscription_items WHERE subscription_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

/// Pending counts for indexer subscriptions, including zeroes so the UI can render every one.
pub(crate) async fn review_summary(pool: &SqlitePool) -> Result<SubscriptionReviewSummary> {
    let rows = sqlx::query_as::<_, ReviewCountRow>(
        "SELECT s.id AS subscription_id, \
                SUM(CASE WHEN i.state = 'pending' THEN 1 ELSE 0 END) AS pending \
         FROM subscriptions s \
         LEFT JOIN subscription_items i ON i.subscription_id = s.id \
         WHERE s.kind = 'indexer' GROUP BY s.id ORDER BY s.name, s.id",
    )
    .fetch_all(pool)
    .await?;
    let subscriptions = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<SubscriptionReviewCount>>>()?;
    let pending_total = subscriptions.iter().map(|entry| entry.pending).sum();
    Ok(SubscriptionReviewSummary {
        pending_total,
        subscriptions,
    })
}

/// Snapshot of all items that are pending at the start of a bulk review action.
pub(crate) async fn pending_item_ids(
    pool: &SqlitePool,
    id: SubscriptionId,
) -> Result<Vec<SubscriptionItemId>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM subscription_items WHERE subscription_id = ? AND state = 'pending' \
         ORDER BY discovered_at, id",
    )
    .bind(id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|value| Ok(SubscriptionItemId::from_uuid(value.parse()?)))
    .collect()
}

/// One item by id, for the review actions that act on a single row.
pub(crate) async fn item(
    pool: &SqlitePool,
    id: rd_core::SubscriptionItemId,
) -> Result<Option<SubscriptionItem>> {
    sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM subscription_items WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn runs(
    pool: &SqlitePool,
    id: SubscriptionId,
    limit: i64,
) -> Result<Vec<SubscriptionRun>> {
    sqlx::query_as::<_, RunRow>(
        "SELECT id, subscription_id, started_at, finished_at, found, accepted, skipped, error \
         FROM subscription_runs WHERE subscription_id = ? ORDER BY started_at DESC LIMIT ?",
    )
    .bind(id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

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
        created_at: now,
        updated_at: now,
    };
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, category_id, priority, \
         interval_seconds, filters_json, backlog_json, category_map_json, \
         source_categories_json, every_release, view, autoplay, card_ratio, schedule, primed, \
         consecutive_failures, secret_ref, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
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
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
    input: NewSubscription,
) -> Result<(Subscription, Option<String>, EventEnvelope)> {
    let existing = sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    ))
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
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE subscriptions SET name = ?, url = ?, kind = ?, enabled = ?, mode = ?, \
         category_id = ?, priority = ?, interval_seconds = ?, filters_json = ?, \
         backlog_json = ?, category_map_json = ?, source_categories_json = ?, \
         every_release = ?, view = ?, autoplay = ?, card_ratio = ?, secret_ref = ?, \
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
    .bind(input.schedule.as_deref())
    .bind(input.schedule.as_deref())
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let updated = sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    ))
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
    let updated = sqlx::query_as::<_, SubscriptionRow>(&format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    ))
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

/// Archives the items of one poll and returns the ones that were genuinely new.
///
/// `INSERT … ON CONFLICT DO NOTHING` against the UNIQUE index is what makes this safe to
/// repeat: an interrupted poll, an overlapping one, or a feed that re-lists the same entry
/// produces no second row and no second download. The returned list is exactly the rows
/// this call created, so the caller queues only those.
pub(crate) async fn record_items(
    connection: &mut SqliteConnection,
    subscription_id: SubscriptionId,
    items: Vec<NewSubscriptionItem>,
) -> Result<(Vec<SubscriptionItem>, EventEnvelope)> {
    let now = Utc::now();
    let mut created = Vec::new();
    let event = changed_event();
    let mut tx = connection.begin().await?;
    for item in items {
        let id = SubscriptionItemId::new();
        let result = sqlx::query(
            "INSERT INTO subscription_items (id, subscription_id, item_key, title, url, \
             published_at, duration_seconds, state, reason, source_category, media_type, \
             attributes_json, password, discovered_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(subscription_id, item_key) DO UPDATE SET \
               url = excluded.url, \
               media_type = COALESCE(excluded.media_type, subscription_items.media_type), \
               attributes_json = \
                 COALESCE(excluded.attributes_json, subscription_items.attributes_json), \
               password = COALESCE(excluded.password, subscription_items.password) \
             WHERE subscription_items.state = 'pending' \
             RETURNING id",
        )
        .bind(id.to_string())
        .bind(subscription_id.to_string())
        .bind(&item.item_key)
        .bind(&item.title)
        .bind(item.url.as_str())
        .bind(item.published_at)
        .bind(item.duration_seconds.map(i64::from))
        .bind(item_state_string(item.state))
        .bind(item.reason.map(reason_string))
        .bind(item.source_category.as_deref())
        .bind(item.media_type.as_deref())
        // `None` rather than an empty object, so the COALESCE above cannot blank a refreshed
        // row's attributes when a later poll answers without the extended block.
        .bind(attributes_json(&item.attributes))
        .bind(item.password.as_deref())
        .bind(now)
        // The returned id tells an insert from a refresh: an update keeps the row's own id,
        // so only a match on the one just generated is a genuinely new item. `rows_affected`
        // cannot make that distinction and would report every refreshed item as discovered.
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| sqlx::Row::get::<String, _>(&row, "id"));
        if result.as_deref() == Some(id.to_string().as_str()) {
            created.push(SubscriptionItem {
                id,
                subscription_id,
                item_key: item.item_key,
                title: item.title,
                url: item.url,
                published_at: item.published_at,
                duration_seconds: item.duration_seconds,
                state: item.state,
                reason: item.reason,
                source_category: item.source_category,
                media_type: item.media_type,
                attributes: item.attributes,
                password: item.password,
                discovered_at: now,
            });
        }
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((created, event))
}

pub(crate) async fn set_item_state(
    connection: &mut SqliteConnection,
    id: SubscriptionItemId,
    state: SubscriptionItemState,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let result = sqlx::query("UPDATE subscription_items SET state = ? WHERE id = ?")
        .bind(item_state_string(state))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("subscription item not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Dismisses a snapshot of pending items in one transaction.
pub(crate) async fn set_pending_items_state(
    connection: &mut SqliteConnection,
    ids: &[SubscriptionItemId],
    state: SubscriptionItemState,
) -> Result<(u64, EventEnvelope)> {
    if ids.is_empty() {
        return Ok((0, changed_event()));
    }
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let mut updated = 0;
    for id in ids {
        updated += sqlx::query(
            "UPDATE subscription_items SET state = ? WHERE id = ? AND state = 'pending'",
        )
        .bind(item_state_string(state))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((updated, event))
}

/// Deletes settled items and every poll run of one subscription, leaving pending review intact.
pub(crate) async fn clear_history(
    connection: &mut SqliteConnection,
    id: SubscriptionId,
) -> Result<(SubscriptionHistoryClearResponse, EventEnvelope)> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM subscriptions WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?;
    if exists == 0 {
        bail!(StoreError::not_found("subscription not found"));
    }
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let deleted_items = sqlx::query(
        "DELETE FROM subscription_items WHERE subscription_id = ? AND state != 'pending'",
    )
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let deleted_runs = sqlx::query("DELETE FROM subscription_runs WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((
        SubscriptionHistoryClearResponse {
            deleted_items,
            deleted_runs,
        },
        event,
    ))
}

/// Writes the poll's outcome and the next due time in one transaction.
///
/// `primed` is set here and never cleared: after one completed poll the backlog decision has
/// been made, and re-applying it later would silently discard everything published since.
pub(crate) async fn finish_run(
    connection: &mut SqliteConnection,
    subscription_id: SubscriptionId,
    started_at: DateTime<Utc>,
    result: PollResult,
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let event = run_finished_event(subscription_id, &result);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO subscription_runs (id, subscription_id, started_at, finished_at, found, \
         accepted, skipped, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(SubscriptionRunId::new().to_string())
    .bind(subscription_id.to_string())
    .bind(started_at)
    .bind(now)
    .bind(i64::from(result.found))
    .bind(i64::from(result.accepted))
    .bind(i64::from(result.skipped))
    .bind(result.error.as_deref())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE subscriptions SET last_run_at = ?, next_run_at = ?, consecutive_failures = ?, \
         last_error = ?, etag = COALESCE(?, etag), last_modified = COALESCE(?, last_modified), \
         primed = 1, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(result.next_run_at)
    .bind(i64::from(result.consecutive_failures))
    .bind(result.error.as_deref())
    .bind(result.etag.as_deref())
    .bind(result.last_modified.as_deref())
    .bind(now)
    .bind(subscription_id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

const fn kind_string(kind: SubscriptionKind) -> &'static str {
    match kind {
        SubscriptionKind::Media => "media",
        SubscriptionKind::Gallery => "gallery",
        SubscriptionKind::Feed => "feed",
        SubscriptionKind::Indexer => "indexer",
        SubscriptionKind::SiteRule => "site_rule",
        SubscriptionKind::Script => "script",
    }
}

const fn mode_string(mode: SubscriptionMode) -> &'static str {
    match mode {
        SubscriptionMode::Review => "review",
        SubscriptionMode::AutoQueue => "auto_queue",
    }
}

const fn item_state_string(state: SubscriptionItemState) -> &'static str {
    match state {
        SubscriptionItemState::Pending => "pending",
        SubscriptionItemState::Queued => "queued",
        SubscriptionItemState::Skipped => "skipped",
        SubscriptionItemState::Dismissed => "dismissed",
    }
}

const fn reason_string(reason: FilterReason) -> &'static str {
    match reason {
        FilterReason::TitleNotIncluded => "title_not_included",
        FilterReason::TitleExcluded => "title_excluded",
        FilterReason::TooShort => "too_short",
        FilterReason::TooLong => "too_long",
        FilterReason::TooOld => "too_old",
        FilterReason::LanguageNotWanted => "language_not_wanted",
        FilterReason::ResolutionTooLow => "resolution_too_low",
        FilterReason::Backlog => "backlog",
    }
}

#[derive(FromRow)]
struct SubscriptionRow {
    id: String,
    name: String,
    url: String,
    kind: String,
    enabled: i64,
    mode: String,
    category_id: Option<String>,
    priority: i64,
    interval_seconds: i64,
    filters_json: Option<String>,
    backlog_json: Option<String>,
    category_map_json: Option<String>,
    source_categories_json: Option<String>,
    every_release: i64,
    view: String,
    autoplay: i64,
    card_ratio: String,
    schedule: Option<String>,
    primed: i64,
    last_run_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    consecutive_failures: i64,
    last_error: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    secret_ref: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<SubscriptionRow> for Subscription {
    type Error = anyhow::Error;

    fn try_from(row: SubscriptionRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionId::from_uuid(row.id.parse()?),
            name: row.name,
            url: Url::parse(&row.url)?,
            kind: match row.kind.as_str() {
                "gallery" => SubscriptionKind::Gallery,
                "feed" => SubscriptionKind::Feed,
                "indexer" => SubscriptionKind::Indexer,
                "site_rule" => SubscriptionKind::SiteRule,
                "script" => SubscriptionKind::Script,
                _ => SubscriptionKind::Media,
            },
            enabled: row.enabled != 0,
            mode: if row.mode == "auto_queue" {
                SubscriptionMode::AutoQueue
            } else {
                SubscriptionMode::Review
            },
            category_id: row
                .category_id
                .map(|value| value.parse().map(CategoryId::from_uuid))
                .transpose()?,
            priority: DownloadPriority::from_i32(i32::try_from(row.priority).unwrap_or_default()),
            interval_seconds: u32::try_from(row.interval_seconds).unwrap_or_default(),
            // The one field that must not fall back: an empty filter set means "accept
            // everything", so a blob that fails to parse would silently turn an exclusion
            // filter into a subscription that queues the entire feed. Fail the row instead —
            // a corrupt `url` or `id` already does, and a subscription nobody can read must
            // not poll. A NULL column is not corruption: it predates the column and never
            // filtered anything.
            filters: match row.filters_json.as_deref() {
                None => Default::default(),
                Some(value) => serde_json::from_str(value).map_err(|error| {
                    tracing::warn!(
                        subscription = %row.id,
                        %error,
                        "subscription filters are unreadable; the subscription is refused \
                         rather than polled without them"
                    );
                    anyhow::Error::new(error).context("subscription filters are unreadable")
                })?,
            },
            // A blob written by a newer version deserialises as far as it can and the rest
            // defaults, rather than making the whole subscription unreadable.
            backlog: row
                .backlog_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            category_map: row
                .category_map_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            // NULL for every row written before the column existed; empty means "everything",
            // so an older subscription keeps asking for exactly what it always did.
            source_categories: row
                .source_categories_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            primed: row.primed != 0,
            last_run_at: row.last_run_at,
            next_run_at: row.next_run_at,
            consecutive_failures: u32::try_from(row.consecutive_failures).unwrap_or_default(),
            last_error: row.last_error,
            etag: row.etag,
            last_modified: row.last_modified,
            has_secret: row.secret_ref.is_some(),
            secret_ref: row.secret_ref,
            every_release: row.every_release != 0,
            view: SubscriptionView::from_stored(&row.view),
            autoplay: row.autoplay != 0,
            card_ratio: SubscriptionCardRatio::from_stored(&row.card_ratio),
            schedule: row.schedule,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct ItemRow {
    id: String,
    subscription_id: String,
    item_key: String,
    title: String,
    url: String,
    published_at: Option<DateTime<Utc>>,
    duration_seconds: Option<i64>,
    state: String,
    reason: Option<String>,
    source_category: Option<String>,
    media_type: Option<String>,
    attributes_json: Option<String>,
    password: Option<String>,
    discovered_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ItemCountRow {
    pending: i64,
    queued: i64,
    skipped: i64,
    dismissed: i64,
}

impl From<ItemCountRow> for SubscriptionItemCounts {
    fn from(row: ItemCountRow) -> Self {
        Self {
            pending: u64::try_from(row.pending).unwrap_or_default(),
            queued: u64::try_from(row.queued).unwrap_or_default(),
            skipped: u64::try_from(row.skipped).unwrap_or_default(),
            dismissed: u64::try_from(row.dismissed).unwrap_or_default(),
        }
    }
}

#[derive(FromRow)]
struct ReviewCountRow {
    subscription_id: String,
    pending: i64,
}

impl TryFrom<ReviewCountRow> for SubscriptionReviewCount {
    type Error = anyhow::Error;

    fn try_from(row: ReviewCountRow) -> Result<Self> {
        Ok(Self {
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            pending: u64::try_from(row.pending).unwrap_or_default(),
        })
    }
}

impl TryFrom<ItemRow> for SubscriptionItem {
    type Error = anyhow::Error;

    fn try_from(row: ItemRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionItemId::from_uuid(row.id.parse()?),
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            item_key: row.item_key,
            title: row.title,
            url: Url::parse(&row.url)?,
            published_at: row.published_at,
            duration_seconds: row
                .duration_seconds
                .and_then(|value| u32::try_from(value).ok()),
            state: match row.state.as_str() {
                "queued" => SubscriptionItemState::Queued,
                "skipped" => SubscriptionItemState::Skipped,
                "dismissed" => SubscriptionItemState::Dismissed,
                _ => SubscriptionItemState::Pending,
            },
            reason: row.reason.as_deref().and_then(|value| match value {
                "title_not_included" => Some(FilterReason::TitleNotIncluded),
                "title_excluded" => Some(FilterReason::TitleExcluded),
                "too_short" => Some(FilterReason::TooShort),
                "too_long" => Some(FilterReason::TooLong),
                "too_old" => Some(FilterReason::TooOld),
                "language_not_wanted" => Some(FilterReason::LanguageNotWanted),
                "resolution_too_low" => Some(FilterReason::ResolutionTooLow),
                "backlog" => Some(FilterReason::Backlog),
                _ => None,
            }),
            source_category: row.source_category,
            media_type: row.media_type,
            // A row written before RD-101-17, or one whose stored JSON no longer parses, is
            // an item without details rather than a row that fails to load.
            attributes: row
                .attributes_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            password: row.password,
            discovered_at: row.discovered_at,
        })
    }
}

#[derive(FromRow)]
struct RunRow {
    id: String,
    subscription_id: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    found: i64,
    accepted: i64,
    skipped: i64,
    error: Option<String>,
}

impl TryFrom<RunRow> for SubscriptionRun {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionRunId::from_uuid(row.id.parse()?),
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            started_at: row.started_at,
            finished_at: row.finished_at,
            found: u32::try_from(row.found).unwrap_or_default(),
            accepted: u32::try_from(row.accepted).unwrap_or_default(),
            skipped: u32::try_from(row.skipped).unwrap_or_default(),
            error: row.error,
        })
    }
}
