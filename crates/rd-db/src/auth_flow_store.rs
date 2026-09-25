//! Persistence of provider authentication flows (RD-090-13).
//!
//! One row per account. The flow lives here rather than in memory so it survives a restart:
//! the sweep loop reads what is due, and the interface reads state rather than driving it.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{AccountId, AuthFlow, AuthFlowState, EventEnvelope, EventKind};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// A flow being started or advanced.
#[derive(Clone, Debug)]
pub struct UpsertAuthFlow {
    pub account_id: AccountId,
    pub plugin_id: String,
    pub state: AuthFlowState,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
    /// When the access token stops working, for a flow that produced one with an expiry.
    pub token_expires_at: Option<DateTime<Utc>>,
    /// Vault reference to the material that mints the next access token. Never returned.
    pub refresh_ref: Option<String>,
    /// Vault reference to the access token, for a provider that keeps it here. Never returned.
    pub access_ref: Option<String>,
    /// Vault reference to the key material a sign-in left beside that token (RD-120-30).
    /// Never returned, and never read by anything but a key derivation.
    pub key_ref: Option<String>,
    /// What the provider must echo back on the redirect. Never returned.
    pub callback_state: Option<String>,
    /// The plugin's own bookkeeping for the next poll. Stored verbatim, never returned.
    pub flow_state: Option<String>,
}

/// Creates or advances the flow of one account.
pub(crate) async fn upsert(
    connection: &mut SqliteConnection,
    input: UpsertAuthFlow,
) -> Result<(AuthFlow, EventEnvelope)> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO auth_flows \
         (account_id, plugin_id, state, verification_url, user_code, expires_at, next_poll_at, \
          message, started_at, token_expires_at, refresh_ref, access_ref, key_ref, \
          callback_state, flow_state) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(account_id) DO UPDATE SET \
         plugin_id = excluded.plugin_id, state = excluded.state, \
         verification_url = excluded.verification_url, user_code = excluded.user_code, \
         expires_at = excluded.expires_at, next_poll_at = excluded.next_poll_at, \
         message = excluded.message, token_expires_at = excluded.token_expires_at, \
         refresh_ref = excluded.refresh_ref, access_ref = excluded.access_ref, \
         key_ref = excluded.key_ref, callback_state = excluded.callback_state, \
         flow_state = excluded.flow_state",
    )
    .bind(input.account_id.to_string())
    .bind(&input.plugin_id)
    .bind(state_string(input.state)?)
    .bind(&input.verification_url)
    .bind(&input.user_code)
    .bind(input.expires_at)
    .bind(input.next_poll_at)
    .bind(&input.message)
    .bind(now)
    .bind(input.token_expires_at)
    .bind(&input.refresh_ref)
    .bind(&input.access_ref)
    .bind(&input.key_ref)
    .bind(&input.callback_state)
    .bind(&input.flow_state)
    .execute(&mut *connection)
    .await?;
    let flow = AuthFlow {
        account_id: input.account_id,
        plugin_id: input.plugin_id,
        state: input.state,
        verification_url: input.verification_url,
        user_code: input.user_code,
        expires_at: input.expires_at,
        next_poll_at: input.next_poll_at,
        message: input.message,
        started_at: now,
        token_expires_at: input.token_expires_at,
        refresh_ref: input.refresh_ref,
        access_ref: input.access_ref,
        key_ref: input.key_ref,
        callback_state: input.callback_state,
        flow_state: input.flow_state,
    };
    let event = EventEnvelope::new(
        EventKind::AccountChanged,
        serde_json::json!({
            "entity": "auth_flow",
            "account_id": flow.account_id,
            "state": flow.state,
        }),
    );
    Ok((flow, event))
}

/// Removes the flow of one account, if it has one.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    account_id: AccountId,
) -> Result<EventEnvelope> {
    sqlx::query("DELETE FROM auth_flows WHERE account_id = ?")
        .bind(account_id.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(EventEnvelope::new(
        EventKind::AccountChanged,
        serde_json::json!({ "entity": "auth_flow", "account_id": account_id }),
    ))
}

/// The flow of one account.
pub(crate) async fn get(pool: &SqlitePool, account_id: AccountId) -> Result<Option<AuthFlow>> {
    let row = sqlx::query_as::<_, FlowRow>(&format!("{SELECT} WHERE account_id = ?"))
        .bind(account_id.to_string())
        .fetch_optional(pool)
        .await?;
    row.map(TryInto::try_into).transpose()
}

/// Every open flow whose next poll is due, oldest first.
///
/// A flow holding a callback state is left out: it is waiting for a browser to come back, not
/// for us to ask again, and polling it would drive it through the device-flow plugins -- which
/// do not claim its provider and would fail it on the first tick.
pub(crate) async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<AuthFlow>> {
    sqlx::query_as::<_, FlowRow>(&format!(
        "{SELECT} WHERE state IN ('waiting_for_user', 'polling') AND callback_state IS NULL \
         AND (next_poll_at IS NULL OR next_poll_at <= ?) ORDER BY started_at"
    ))
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Records what a renewal produced: when the new token dies, and what mints the one after it.
///
/// An update rather than an upsert, because the caller knows only these two things and an
/// upsert would make it restate the whole row -- including the state and the bookkeeping it
/// has no business touching -- just to change them.
pub(crate) async fn set_renewal(
    connection: &mut SqliteConnection,
    account_id: AccountId,
    token_expires_at: Option<DateTime<Utc>>,
    refresh_ref: Option<&str>,
    access_ref: Option<&str>,
) -> Result<EventEnvelope> {
    // A new token drops the key that belonged to the old one (RD-120-30): the two are halves
    // of one session, and a renewal never produces key material of its own.
    sqlx::query(
        "UPDATE auth_flows SET token_expires_at = ?, refresh_ref = ?, \
         key_ref = CASE WHEN ? IS NULL THEN key_ref ELSE NULL END, \
         access_ref = COALESCE(?, access_ref) WHERE account_id = ?",
    )
    .bind(token_expires_at)
    .bind(refresh_ref)
    .bind(access_ref)
    .bind(access_ref)
    .bind(account_id.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(EventEnvelope::new(
        EventKind::AccountChanged,
        serde_json::json!({ "entity": "auth_flow", "account_id": account_id }),
    ))
}

/// Records the session a sign-in stored beside its account's own credential (RD-120-30):
/// the token under `access_ref`, the key material under `key_ref`, both at once.
///
/// One statement, so the two halves never belong to different sessions: an interruption
/// before it leaves the old pair, after it the new pair, and never a new token with an old
/// key. An upsert rather than an update, because a sign-in that finishes inside its first
/// call stores its session before the flow service has written a row at all; the row it
/// creates says `authorized`, which is true, and the service's own upsert, which follows in
/// the same call, names the plugin.
pub(crate) async fn set_session(
    connection: &mut SqliteConnection,
    account_id: AccountId,
    access_ref: &str,
    key_ref: Option<&str>,
) -> Result<EventEnvelope> {
    sqlx::query(
        "INSERT INTO auth_flows (account_id, plugin_id, state, started_at, access_ref, key_ref) \
         VALUES (?, '', 'authorized', ?, ?, ?) \
         ON CONFLICT(account_id) DO UPDATE SET \
         access_ref = excluded.access_ref, key_ref = excluded.key_ref",
    )
    .bind(account_id.to_string())
    .bind(Utc::now())
    .bind(access_ref)
    .bind(key_ref)
    .execute(&mut *connection)
    .await?;
    Ok(EventEnvelope::new(
        EventKind::AccountChanged,
        serde_json::json!({ "entity": "auth_flow", "account_id": account_id }),
    ))
}

/// The flow an arriving callback belongs to, found by the value the provider echoed back.
///
/// The lookup *is* the check: a callback carrying a state no open flow claims matches nothing
/// and is refused, which is what the state is for.
pub(crate) async fn by_callback_state(
    pool: &SqlitePool,
    callback_state: &str,
) -> Result<Option<AuthFlow>> {
    let row = sqlx::query_as::<_, FlowRow>(&format!("{SELECT} WHERE callback_state = ?"))
        .bind(callback_state)
        .fetch_optional(pool)
        .await?;
    row.map(TryInto::try_into).transpose()
}

/// Holds a renewal back until `next_poll_at`.
///
/// `next_poll_at` is free on an authorised row -- the sign-in sweep only ever looks at rows
/// waiting for a person or being polled -- and it already means "when the provider is asked
/// next", which is exactly what a deferred renewal needs. Without it a provider that answers
/// "not yet" would be asked again every three seconds, which is how an application earns a
/// rate limit.
pub(crate) async fn defer_renewal(
    connection: &mut SqliteConnection,
    account_id: AccountId,
    next_poll_at: DateTime<Utc>,
) -> Result<EventEnvelope> {
    sqlx::query("UPDATE auth_flows SET next_poll_at = ? WHERE account_id = ?")
        .bind(next_poll_at)
        .bind(account_id.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(EventEnvelope::new(
        EventKind::AccountChanged,
        serde_json::json!({ "entity": "auth_flow", "account_id": account_id }),
    ))
}

/// Every authorised flow whose access token is due for renewal by `threshold`, soonest first.
///
/// Deliberately not folded into `due`: that one advances a sign-in somebody is waiting on, this
/// one renews behind their back, and the two share neither their candidate rows nor their order.
pub(crate) async fn due_refresh(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    threshold: DateTime<Utc>,
) -> Result<Vec<AuthFlow>> {
    sqlx::query_as::<_, FlowRow>(&format!(
        "{SELECT} WHERE state = 'authorized' AND refresh_ref IS NOT NULL \
         AND token_expires_at IS NOT NULL AND token_expires_at <= ? \
         AND (next_poll_at IS NULL OR next_poll_at <= ?) ORDER BY token_expires_at"
    ))
    .bind(threshold)
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

const SELECT: &str = "SELECT account_id, plugin_id, state, verification_url, user_code, \
     expires_at, next_poll_at, message, started_at, token_expires_at, refresh_ref, \
     access_ref, key_ref, callback_state, flow_state FROM auth_flows";

#[derive(FromRow)]
struct FlowRow {
    account_id: String,
    plugin_id: String,
    state: String,
    verification_url: Option<String>,
    user_code: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    next_poll_at: Option<DateTime<Utc>>,
    message: Option<String>,
    started_at: DateTime<Utc>,
    token_expires_at: Option<DateTime<Utc>>,
    refresh_ref: Option<String>,
    access_ref: Option<String>,
    key_ref: Option<String>,
    callback_state: Option<String>,
    flow_state: Option<String>,
}

impl TryFrom<FlowRow> for AuthFlow {
    type Error = anyhow::Error;

    fn try_from(row: FlowRow) -> Result<Self> {
        Ok(Self {
            account_id: row.account_id.parse()?,
            plugin_id: row.plugin_id,
            state: serde_json::from_str(&format!("\"{}\"", row.state))?,
            verification_url: row.verification_url,
            user_code: row.user_code,
            expires_at: row.expires_at,
            next_poll_at: row.next_poll_at,
            message: row.message,
            started_at: row.started_at,
            token_expires_at: row.token_expires_at,
            refresh_ref: row.refresh_ref,
            access_ref: row.access_ref,
            key_ref: row.key_ref,
            callback_state: row.callback_state,
            flow_state: row.flow_state,
        })
    }
}

fn state_string(state: AuthFlowState) -> Result<String> {
    Ok(serde_json::to_string(&state)?.trim_matches('"').to_owned())
}
