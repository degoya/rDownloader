use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{
    Account, AccountId, AuthProfile, AuthProfileSelection, EventEnvelope, EventKind, ProxyKind,
    ProxyProfile, ProxyProfileId,
};
use rd_provider_registry::CredentialMode;
use sqlx::{Connection, FromRow, Row, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, parse_id, writer::insert_event};

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

pub(crate) async fn delete_account(
    connection: &mut SqliteConnection,
    id: AccountId,
) -> Result<((Option<String>, Option<String>), EventEnvelope)> {
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
    Ok(((row.get("secret_ref"), row.get("cookie_ref")), event))
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
        let used = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT COUNT(*) FROM {table} WHERE proxy_profile_id = ?{condition}"
        ))
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

pub(crate) async fn list_accounts(pool: &SqlitePool) -> Result<Vec<Account>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT id, provider, label, username, credential_mode, proxy_profile_id, enabled, \
         secret_ref IS NOT NULL AS has_secret, cookie_ref IS NOT NULL AS has_cookies \
         FROM accounts ORDER BY provider, label",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn account_secret_refs(
    pool: &SqlitePool,
    id: AccountId,
) -> Result<Option<(Option<String>, Option<String>)>> {
    let row = sqlx::query_as::<_, AccountSecretRow>(
        "SELECT secret_ref, cookie_ref FROM accounts WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| (row.secret_ref, row.cookie_ref)))
}

pub(crate) async fn list_proxy_profiles(pool: &SqlitePool) -> Result<Vec<ProxyProfile>> {
    sqlx::query_as::<_, ProxyRow>(
        "SELECT id, name, kind, endpoint, username, secret_ref, \
         secret_ref IS NOT NULL AS has_credentials FROM proxy_profiles ORDER BY name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn client_config(
    pool: &SqlitePool,
    account_id: Option<AccountId>,
    job_proxy_id: Option<ProxyProfileId>,
    global_proxy_id: Option<ProxyProfileId>,
    selection: AuthProfileSelection,
    url: &Url,
) -> Result<NetworkClientConfig> {
    let account = match account_id {
        Some(id) => Some(
            sqlx::query_as::<_, AccountNetworkRow>(
                "SELECT provider, username, secret_ref, cookie_ref, proxy_profile_id, enabled \
                 FROM accounts WHERE id = ?",
            )
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?
            .context("selected account not found")?,
        ),
        None => None,
    };
    if account.as_ref().is_some_and(|account| !account.enabled) {
        anyhow::bail!("selected account is disabled");
    }
    let account_proxy_id = account
        .as_ref()
        .and_then(|account| account.proxy_profile_id.as_deref())
        .map(parse_id)
        .transpose()?;
    let proxy_id = job_proxy_id.or(account_proxy_id).or(global_proxy_id);
    let proxy = match proxy_id {
        Some(id) => Some(
            load_proxy(pool, id)
                .await?
                .context("selected proxy profile not found")?,
        ),
        None => None,
    };
    // An account already carries its own scoped cookie jar; layering a domain profile on
    // top would mix two sessions for the same host. The account wins.
    let auth = if account.is_some() {
        None
    } else {
        resolve_auth_profile(pool, selection, url).await?
    };
    Ok(NetworkClientConfig {
        account_id,
        account_provider: account.as_ref().map(|account| account.provider.clone()),
        account_username: account
            .as_ref()
            .and_then(|account| account.username.clone()),
        account_secret_ref: account
            .as_ref()
            .and_then(|account| account.secret_ref.clone()),
        cookie_ref: account
            .as_ref()
            .and_then(|account| account.cookie_ref.clone()),
        proxy,
        auth,
    })
}

/// Resolves the selection into a usable profile.
///
/// The two modes fail differently on purpose. A pinned profile that is gone, disabled or
/// expired is a hard error: continuing unauthenticated would quietly write a login page
/// over the user's file. Auto-matching is a convenience, so it just finds nothing.
async fn resolve_auth_profile(
    pool: &SqlitePool,
    selection: AuthProfileSelection,
    url: &Url,
) -> Result<Option<AuthProfile>> {
    match selection {
        AuthProfileSelection::None => Ok(None),
        AuthProfileSelection::Auto => crate::auth_profile_store::match_for_url(pool, url).await,
        AuthProfileSelection::Pinned(id) => {
            let profile = crate::auth_profile_store::get(pool, id)
                .await?
                .context("pinned auth profile not found")?;
            if !profile.enabled {
                anyhow::bail!("pinned auth profile is disabled");
            }
            if profile.is_expired(Utc::now()) {
                anyhow::bail!("pinned auth profile has expired");
            }
            if !profile.scope.matches_url(url) {
                anyhow::bail!("pinned auth profile does not cover this URL");
            }
            Ok(Some(profile))
        }
    }
}

pub(crate) async fn load_proxy(
    pool: &SqlitePool,
    id: ProxyProfileId,
) -> Result<Option<ProxyProfile>> {
    sqlx::query_as::<_, ProxyRow>(
        "SELECT id, name, kind, endpoint, username, secret_ref, \
         secret_ref IS NOT NULL AS has_credentials FROM proxy_profiles WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

#[derive(FromRow)]
struct AccountRow {
    id: String,
    provider: String,
    label: String,
    username: Option<String>,
    credential_mode: Option<String>,
    proxy_profile_id: Option<String>,
    enabled: bool,
    has_secret: bool,
    has_cookies: bool,
}

#[derive(FromRow)]
struct AccountNetworkRow {
    provider: String,
    username: Option<String>,
    secret_ref: Option<String>,
    cookie_ref: Option<String>,
    proxy_profile_id: Option<String>,
    enabled: bool,
}

#[derive(FromRow)]
struct AccountSecretRow {
    secret_ref: Option<String>,
    cookie_ref: Option<String>,
}

impl TryFrom<AccountRow> for Account {
    type Error = anyhow::Error;

    fn try_from(row: AccountRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            provider: row.provider,
            label: row.label,
            username: row.username,
            // Refused rather than defaulted: an unreadable mode would otherwise silently
            // become the first declared one, which is the difference between sending a
            // password to the website and sending an API key to the API host.
            credential_mode: row
                .credential_mode
                .as_deref()
                .map(|value| {
                    CredentialMode::parse(value)
                        .with_context(|| format!("unknown account credential mode {value}"))
                })
                .transpose()?,
            proxy_profile_id: row.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            enabled: row.enabled,
            has_secret: row.has_secret,
            has_cookies: row.has_cookies,
        })
    }
}

#[derive(FromRow)]
struct ProxyRow {
    id: String,
    name: String,
    kind: String,
    endpoint: String,
    username: Option<String>,
    secret_ref: Option<String>,
    has_credentials: bool,
}

impl TryFrom<ProxyRow> for ProxyProfile {
    type Error = anyhow::Error;

    fn try_from(row: ProxyRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            kind: parse_enum(&row.kind)?,
            endpoint: Url::parse(&row.endpoint).context("parse stored proxy endpoint")?,
            username: row.username,
            secret_ref: row.secret_ref,
            has_credentials: row.has_credentials,
        })
    }
}

fn network_event<T: serde::Serialize>(kind: EventKind, resource: &str, id: T) -> EventEnvelope {
    EventEnvelope::new(kind, serde_json::json!({ "resource": resource, "id": id }))
}

pub(crate) fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

pub(crate) fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).context("parse stored enum")
}
