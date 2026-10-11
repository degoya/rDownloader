//! Web Push subscriptions and the VAPID key (RD-1240-13). See `migrations/0137_web_push.sql`.
//!
//! A browser's subscription is named by its push address: subscribing again updates the row. The
//! first subscription also gives the hub something to deliver through — a `web_push` target and
//! a rule for every event — in the same transaction, so turning push on in a browser is enough;
//! each browser's own choice of events then filters what reaches it. A target deleted later is
//! made again by the next subscription.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, NotificationRuleId, NotificationTargetId};
use rd_notify::{NotificationEvent, WebPushSubscription};
use sqlx::{Connection, FromRow, SqliteConnection};

use crate::{
    Database, commands::NotifyCommand, error::StoreError, notify_store::changed_event,
    writer::insert_event,
};

/// The name the first subscription gives the target it makes; a number follows when the name
/// is taken by a target of another kind.
const TARGET_NAME: &str = "Browser push";

/// What a browser hands over when it subscribes.
#[derive(Clone, Debug)]
pub struct NewWebPushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_name: String,
    pub events: Vec<NotificationEvent>,
}

/// The VAPID key in force: its vault reference and its public half.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebPushKey {
    pub private_key_ref: String,
    pub public_key: String,
}

#[derive(FromRow)]
struct SubscriptionRow {
    id: String,
    endpoint: String,
    p256dh: String,
    auth: String,
    device_name: String,
    events_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<SubscriptionRow> for WebPushSubscription {
    type Error = anyhow::Error;

    fn try_from(row: SubscriptionRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            endpoint: row.endpoint,
            device_name: row.device_name,
            events: serde_json::from_str(&row.events_json).context("stored push events")?,
            created_at: row.created_at,
            updated_at: row.updated_at,
            p256dh: row.p256dh,
            auth: row.auth,
        })
    }
}

const COLUMNS: &str =
    "id, endpoint, p256dh, auth, device_name, events_json, created_at, updated_at";

impl Database {
    /// Every browser that receives push messages, the oldest first.
    pub async fn list_web_push_subscriptions(&self) -> Result<Vec<WebPushSubscription>> {
        sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM web_push_subscriptions ORDER BY created_at, id"
        )))
        .fetch_all(&self.readers)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
    }

    /// The VAPID key in force, if one was made.
    pub async fn web_push_key(&self) -> Result<Option<WebPushKey>> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT private_key_ref, public_key FROM web_push_keys WHERE slot = 1")
                .fetch_optional(&self.readers)
                .await?;
        Ok(row.map(|(private_key_ref, public_key)| WebPushKey {
            private_key_ref,
            public_key,
        }))
    }

    /// Stores a browser's subscription, or updates the one with the same push address; makes
    /// the `web_push` target and its rule when there is none.
    pub async fn upsert_web_push_subscription(
        &self,
        input: NewWebPushSubscription,
    ) -> Result<WebPushSubscription> {
        crate::writer::request(&self.writer, |reply| {
            NotifyCommand::UpsertWebPushSubscription { input, reply }
        })
        .await
    }

    /// Deletes one subscription; a missing one is a not-found error.
    pub async fn delete_web_push_subscription(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        crate::writer::request(&self.writer, |reply| {
            NotifyCommand::DeleteWebPushSubscription { id, reply }
        })
        .await
    }

    /// Records a new VAPID key and answers the one in force.
    ///
    /// Without `replacing`, a key already stored wins — two first requests at once keep the
    /// first, and the caller drops its own vault entry. With `replacing`, the key whose vault
    /// entry is unreadable (a backup restored on another machine) is replaced if it is still the
    /// one in force, and every subscription goes with it: they were made for the old key, and a
    /// push service refuses a message signed with another one.
    pub async fn store_web_push_key(
        &self,
        key: WebPushKey,
        replacing: Option<String>,
    ) -> Result<WebPushKey> {
        crate::writer::request(&self.writer, |reply| NotifyCommand::StoreWebPushKey {
            key,
            replacing,
            reply,
        })
        .await
    }
}

/// The writer half of [`Database::upsert_web_push_subscription`].
pub(crate) async fn upsert_subscription(
    connection: &mut SqliteConnection,
    input: NewWebPushSubscription,
) -> Result<(WebPushSubscription, EventEnvelope)> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO web_push_subscriptions \
         (id, endpoint, p256dh, auth, device_name, events_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(endpoint) DO UPDATE SET \
           p256dh = excluded.p256dh, \
           auth = excluded.auth, \
           device_name = excluded.device_name, \
           events_json = excluded.events_json, \
           updated_at = excluded.updated_at",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&input.endpoint)
    .bind(&input.p256dh)
    .bind(&input.auth)
    .bind(&input.device_name)
    .bind(serde_json::to_string(&input.events)?)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM web_push_subscriptions WHERE endpoint = ?"
    )))
    .bind(&input.endpoint)
    .fetch_one(&mut *tx)
    .await?;
    let subscription = WebPushSubscription::try_from(row)?;
    ensure_target(&mut tx, now).await?;
    let event = changed_event("web_push_subscription", subscription.id.clone());
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((subscription, event))
}

/// Makes the `web_push` target and a rule for every event, unless a `web_push` target exists.
async fn ensure_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: DateTime<Utc>,
) -> Result<()> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM notification_targets WHERE kind = 'web_push' LIMIT 1")
            .fetch_optional(&mut **tx)
            .await?;
    if existing.is_some() {
        return Ok(());
    }
    let taken: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM notification_targets WHERE name = ? OR name LIKE ? || ' %'",
    )
    .bind(TARGET_NAME)
    .bind(TARGET_NAME)
    .fetch_all(&mut **tx)
    .await?;
    let name = std::iter::once(TARGET_NAME.to_owned())
        .chain((2..).map(|number| format!("{TARGET_NAME} {number}")))
        .find(|name| !taken.contains(name))
        .unwrap_or_else(|| TARGET_NAME.to_owned());
    let target_id = NotificationTargetId::new();
    sqlx::query(
        "INSERT INTO notification_targets (id, name, kind, enabled, endpoint, config_json, \
         secret_ref, created_at, updated_at) VALUES (?, ?, 'web_push', 1, '', '{}', NULL, ?, ?)",
    )
    .bind(target_id.to_string())
    .bind(&name)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO notification_rules (id, name, enabled, target_id, events_json, category_id, \
         min_severity, created_at, updated_at) VALUES (?, ?, 1, ?, '[]', NULL, 'info', ?, ?)",
    )
    .bind(NotificationRuleId::new().to_string())
    .bind(&name)
    .bind(target_id.to_string())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The writer half of [`Database::delete_web_push_subscription`].
pub(crate) async fn delete_subscription(
    connection: &mut SqliteConnection,
    id: String,
) -> Result<EventEnvelope> {
    let event = changed_event("web_push_subscription", id.clone());
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM web_push_subscriptions WHERE id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    anyhow::ensure!(
        deleted.rows_affected() > 0,
        StoreError::not_found("push subscription not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// The writer half of [`Database::store_web_push_key`].
pub(crate) async fn store_key(
    connection: &mut SqliteConnection,
    key: WebPushKey,
    replacing: Option<String>,
) -> Result<WebPushKey> {
    let mut tx = connection.begin().await?;
    match replacing {
        None => {
            sqlx::query(
                "INSERT INTO web_push_keys (slot, private_key_ref, public_key, created_at) \
                 VALUES (1, ?, ?, ?) ON CONFLICT(slot) DO NOTHING",
            )
            .bind(&key.private_key_ref)
            .bind(&key.public_key)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }
        Some(stale) => {
            let replaced = sqlx::query(
                "UPDATE web_push_keys SET private_key_ref = ?, public_key = ?, created_at = ? \
                 WHERE slot = 1 AND private_key_ref = ?",
            )
            .bind(&key.private_key_ref)
            .bind(&key.public_key)
            .bind(Utc::now())
            .bind(&stale)
            .execute(&mut *tx)
            .await?;
            if replaced.rows_affected() > 0 {
                sqlx::query("DELETE FROM web_push_subscriptions")
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    let (private_key_ref, public_key): (String, String) =
        sqlx::query_as("SELECT private_key_ref, public_key FROM web_push_keys WHERE slot = 1")
            .fetch_one(&mut *tx)
            .await
            .context("the VAPID key in force")?;
    tx.commit().await?;
    Ok(WebPushKey {
        private_key_ref,
        public_key,
    })
}

#[cfg(test)]
#[path = "web_push_store_tests.rs"]
mod tests;
