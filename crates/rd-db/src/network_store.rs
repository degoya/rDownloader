use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{
    Account, AccountId, AuthProfile, EventEnvelope, EventKind, ProxyKind, ProxyProfile,
    ProxyProfileId,
};
use rd_provider_registry::CredentialMode;
use sqlx::{Connection, Row, SqliteConnection};
use url::Url;

use crate::{enum_string, error::StoreError, writer::insert_event};

#[path = "network_store_reads.rs"]
mod reads;

pub(crate) use reads::{
    account_secret_refs, client_config, get_account, list_accounts, list_proxy_profiles, load_proxy,
};

#[derive(Clone, Debug)]
pub struct NewAccount {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret_ref` holds, for a provider that offers a choice.
    pub credential_mode: Option<CredentialMode>,
    pub secret_ref: Option<String>,
    pub cookie_ref: Option<String>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct UpdateAccount {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret_ref` holds, for a provider that offers a choice.
    pub credential_mode: Option<CredentialMode>,
    pub secret_ref: Option<String>,
    pub cookie_ref: Option<String>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct NewProxyProfile {
    pub name: String,
    pub kind: ProxyKind,
    pub endpoint: Url,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
}

/// Internal references required to build one isolated HTTP client.
#[derive(Clone, Debug)]
pub struct NetworkClientConfig {
    pub account_id: Option<AccountId>,
    /// Provider slug of the account (e.g. `ddownload`), used to scope its cookies.
    pub account_provider: Option<String>,
    /// The account's user name, which a provider that authenticates a transfer with HTTP Basic
    /// pairs with the secret (RD-120-38). Not a secret: it is stored and shown in the clear.
    pub account_username: Option<String>,
    pub account_secret_ref: Option<String>,
    pub cookie_ref: Option<String>,
    pub proxy: Option<ProxyProfile>,
    /// Auth profile resolved for this job, already scope-checked against its URL.
    pub auth: Option<AuthProfile>,
}

pub(crate) async fn create_account(
    connection: &mut SqliteConnection,
    input: NewAccount,
) -> Result<(Account, EventEnvelope)> {
    let value = Account {
        id: AccountId::new(),
        provider: input.provider,
        label: input.label,
        username: input.username,
        credential_mode: input.credential_mode,
        proxy_profile_id: input.proxy_profile_id,
        enabled: input.enabled,
        has_secret: input.secret_ref.is_some(),
        has_cookies: input.cookie_ref.is_some(),
    };
    let now = Utc::now();
    let event = network_event(EventKind::AccountChanged, "account", value.id);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO accounts (id, provider, label, username, credential_mode, secret_ref, \
         cookie_ref, proxy_profile_id, enabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.provider)
    .bind(&value.label)
    .bind(&value.username)
    .bind(value.credential_mode.map(CredentialMode::as_str))
    .bind(input.secret_ref)
    .bind(input.cookie_ref)
    .bind(value.proxy_profile_id.map(|id| id.to_string()))
    .bind(value.enabled)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update_account(
    connection: &mut SqliteConnection,
    id: AccountId,
    input: UpdateAccount,
) -> Result<(Account, EventEnvelope)> {
    let value = Account {
        id,
        provider: input.provider,
        label: input.label,
        username: input.username,
        credential_mode: input.credential_mode,
        proxy_profile_id: input.proxy_profile_id,
        enabled: input.enabled,
        has_secret: input.secret_ref.is_some(),
        has_cookies: input.cookie_ref.is_some(),
    };
    let event = network_event(EventKind::AccountChanged, "account", value.id);
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE accounts SET provider = ?, label = ?, username = ?, credential_mode = ?, \
         secret_ref = ?, cookie_ref = ?, proxy_profile_id = ?, enabled = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(&value.provider)
    .bind(&value.label)
    .bind(&value.username)
    .bind(value.credential_mode.map(CredentialMode::as_str))
    .bind(input.secret_ref)
    .bind(input.cookie_ref)
    .bind(value.proxy_profile_id.map(|id| id.to_string()))
    .bind(value.enabled)
    .bind(Utc::now())
    .bind(value.id.to_string())
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        StoreError::not_found("account not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Deletes an unused account; answers its own two references and, apart, the ones its sign-in
/// held, whose rows the delete cascades away (DB-02).
pub(crate) async fn delete_account(
    connection: &mut SqliteConnection,
    id: AccountId,
) -> Result<(
    ((Option<String>, Option<String>), Vec<String>),
    EventEnvelope,
)> {
    let event = network_event(EventKind::AccountChanged, "account", id);
    let mut tx = connection.begin().await?;
    let row = sqlx::query("SELECT secret_ref, cookie_ref FROM accounts WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context(StoreError::not_found("account not found"))?;
    let jobs = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM downloads WHERE account_id = ? \
         AND state NOT IN ('completed', 'cancelled')",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    anyhow::ensure!(
        jobs == 0,
        StoreError::in_use("account is still used by download jobs")
    );
    let sign_in = crate::auth_flow_store::sign_in_references(&mut tx, id).await?;
    sqlx::query("UPDATE downloads SET account_id = NULL WHERE account_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((
        ((row.get("secret_ref"), row.get("cookie_ref")), sign_in),
        event,
    ))
}

pub(crate) async fn create_proxy_profile(
    connection: &mut SqliteConnection,
    input: NewProxyProfile,
) -> Result<(ProxyProfile, EventEnvelope)> {
    let value = ProxyProfile {
        id: ProxyProfileId::new(),
        name: input.name,
        kind: input.kind,
        endpoint: input.endpoint,
        username: input.username,
        has_credentials: input.secret_ref.is_some(),
        secret_ref: input.secret_ref,
    };
    let now = Utc::now();
    let event = network_event(EventKind::ProxyChanged, "proxy_profile", value.id);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO proxy_profiles (id, name, kind, endpoint, username, secret_ref, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(enum_string(value.kind)?)
    .bind(value.endpoint.as_str())
    .bind(&value.username)
    .bind(&value.secret_ref)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update_proxy_profile(
    connection: &mut SqliteConnection,
    id: ProxyProfileId,
    input: NewProxyProfile,
) -> Result<(ProxyProfile, EventEnvelope)> {
    let value = ProxyProfile {
        id,
        name: input.name,
        kind: input.kind,
        endpoint: input.endpoint,
        username: input.username,
        has_credentials: input.secret_ref.is_some(),
        secret_ref: input.secret_ref,
    };
    let event = network_event(EventKind::ProxyChanged, "proxy_profile", value.id);
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE proxy_profiles SET name = ?, kind = ?, endpoint = ?, username = ?, \
         secret_ref = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(enum_string(value.kind)?)
    .bind(value.endpoint.as_str())
    .bind(&value.username)
    .bind(&value.secret_ref)
    .bind(Utc::now())
    .bind(value.id.to_string())
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        StoreError::not_found("proxy profile not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// Removes a proxy profile and hands back its credential reference for the vault sweep.
///
/// Refused while anything still points at it. Nulling the column instead would silently move
/// an account, a Usenet server or a running job onto the direct connection — the one change a
/// person routing traffic through a proxy would least want made for them.
pub(crate) async fn delete_proxy_profile(
    connection: &mut SqliteConnection,
    id: ProxyProfileId,
) -> Result<(Option<String>, EventEnvelope)> {
    let event = network_event(EventKind::ProxyChanged, "proxy_profile", id);
    let mut tx = connection.begin().await?;
    let row = sqlx::query("SELECT secret_ref FROM proxy_profiles WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context(StoreError::not_found("proxy profile not found"))?;
    for (table, condition) in [
        ("accounts", ""),
        ("usenet_servers", ""),
        ("downloads", " AND state NOT IN ('completed', 'cancelled')"),
    ] {
        let used = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE proxy_profile_id = ?{condition}"
        )))
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        anyhow::ensure!(
            used == 0,
            StoreError::in_use(format!("proxy profile is still used by {table}"))
        );
    }
    sqlx::query("DELETE FROM proxy_profiles WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let secret_ref: Option<String> = row.get("secret_ref");
    Ok((secret_ref, event))
}

fn network_event<T: serde::Serialize>(kind: EventKind, resource: &str, id: T) -> EventEnvelope {
    EventEnvelope::new(kind, serde_json::json!({ "resource": resource, "id": id }))
}
