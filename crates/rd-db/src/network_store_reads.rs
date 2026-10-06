//! The read side of accounts and proxy profiles: listings, one record, the references a delete
//! sweeps, and the network configuration one job's HTTP client is built from.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{
    Account, AccountId, AuthProfile, AuthProfileSelection, ProxyProfile, ProxyProfileId,
};
use rd_provider_registry::CredentialMode;
use sqlx::{FromRow, SqlitePool};
use url::Url;

use super::NetworkClientConfig;
use crate::{parse_enum, parse_id};

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

/// One account's public metadata by id, read like [`list_accounts`].
pub(crate) async fn get_account(pool: &SqlitePool, id: AccountId) -> Result<Option<Account>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT id, provider, label, username, credential_mode, proxy_profile_id, enabled, \
         secret_ref IS NOT NULL AS has_secret, cookie_ref IS NOT NULL AS has_cookies \
         FROM accounts WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
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
