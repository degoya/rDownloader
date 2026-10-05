//! The clients a transfer runs on, and the credentials that may ride along on its requests.

use std::sync::Arc;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rd_core::{AuthMethod, DownloadFile, Failure};
use rd_http::{AuthMaterial, ClientContext, ClientKey, ProxyCredentials, import_into};
use reqwest::cookie::Jar;
use secrecy::{ExposeSecret, SecretString};

use super::{NetworkClient, ProviderCredential};
use crate::{ProfileBoundary, SchedulerHandle};

/// Builds (or reuses) the isolated client for an account/proxy/profile combination.
/// Reached from outside through [`SchedulerHandle::network_client`].
pub(crate) async fn build_client(
    scheduler: &SchedulerHandle,
    account_id: Option<rd_core::AccountId>,
    proxy_profile_id: Option<rd_core::ProxyProfileId>,
    auth_profile: rd_core::AuthProfileSelection,
    scope: &url::Url,
    address_policy: Option<rd_http::AddressPolicy>,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let config = scheduler
        .database
        .network_client_config(
            account_id,
            proxy_profile_id,
            defaults.global_proxy_profile_id,
            auth_profile,
            scope,
        )
        .await?;
    assemble_client(scheduler, config, defaults, scope, None, address_policy).await
}

/// The transfer client for a download, confined to the replay's approved origins when it
/// has a consented template, and to `address_policy` when its addresses came from a source
/// set (RD-150-03).
pub(crate) async fn build_replay_client(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    replay: Option<&crate::replay::ReplayContext>,
    address_policy: Option<rd_http::AddressPolicy>,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let config = scheduler
        .database
        .network_client_config(
            file.account_id,
            file.proxy_profile_id,
            defaults.global_proxy_profile_id,
            file.auth_profile,
            &file.source,
        )
        .await?;
    let scope = replay
        .and_then(crate::replay::ReplayContext::scope)
        .map(Arc::new);
    assemble_client(
        scheduler,
        config,
        defaults,
        &file.source,
        scope,
        address_policy,
    )
    .await
}

/// Builds a client for one specific profile without consulting the selection rules, so a
/// profile can be tested before it is approved or while it is switched off.
pub(crate) async fn build_test_client(
    scheduler: &SchedulerHandle,
    profile: rd_core::AuthProfile,
    scope: &url::Url,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let mut config = scheduler
        .database
        .network_client_config(
            None,
            None,
            defaults.global_proxy_profile_id,
            rd_core::AuthProfileSelection::None,
            scope,
        )
        .await?;
    config.auth = Some(profile);
    assemble_client(scheduler, config, defaults, scope, None, None).await
}

/// Turns a resolved network configuration into a pooled client plus its credential headers.
async fn assemble_client(
    scheduler: &SchedulerHandle,
    config: rd_db::NetworkClientConfig,
    defaults: rd_http::NetworkDefaults,
    scope: &url::Url,
    replay_scope: Option<Arc<rd_http::ReplayScope>>,
    address_policy: Option<rd_http::AddressPolicy>,
) -> Result<NetworkClient> {
    // Read before `config` is taken apart below; the header itself is built later, and only
    // once the address the transfer actually goes to is known.
    let provider_credential = provider_transfer_credential(scheduler, &config).await?;
    let cookie_jar = Arc::new(Jar::default());
    if let Some(reference) = &config.cookie_ref {
        let content = scheduler.secrets.get(reference).await?;
        let cookie_scope = config
            .account_provider
            .as_deref()
            .and_then(rd_plugin_host::provider_cookie_scope)
            .unwrap_or_else(|| scope.clone());
        let cookie_scope = rd_http::CookieScope::provider(&cookie_scope)?;
        import_into(&cookie_jar, content.expose_secret(), &cookie_scope)?;
    }
    let mut headers = Vec::new();
    let mut profile_boundary = None;
    let mut auth_material = None;
    if let Some(profile) = &config.auth {
        let secret = match &profile.secret_ref {
            Some(reference) => Some(scheduler.secrets.get(reference).await?),
            None => None,
        };
        match profile.method {
            AuthMethod::Cookies => {
                if let Some(secret) = &secret {
                    let cookie_scope = rd_http::CookieScope::new(
                        &profile.scope.probe_url().unwrap_or_else(|| scope.clone()),
                        profile.scope.include_subdomains,
                    )?;
                    import_into(&cookie_jar, secret.expose_secret(), &cookie_scope)?;
                }
            }
            AuthMethod::Basic | AuthMethod::Bearer => {
                if let Some(secret) = &secret {
                    headers.push((
                        "authorization".to_owned(),
                        authorization_value(profile, secret)?,
                    ));
                    profile_boundary = Some(ProfileBoundary::new(profile.scope.clone(), scope));
                }
            }
        }
        if let Some(reference) = &profile.certificate_ref {
            auth_material = Some(AuthMaterial {
                identity_pem: scheduler.secrets.get(reference).await?,
            });
        }
    }
    // Only client-wide material may key the pool; a per-request header must not.
    let client_wide = config
        .auth
        .as_ref()
        .filter(|profile| {
            profile.certificate_ref.is_some() || profile.method == AuthMethod::Cookies
        })
        .map(|profile| (profile.id, profile.revision()));
    let proxy_profile_id = config.proxy.as_ref().map(|profile| profile.id);
    let proxy_credentials = match config.proxy.as_ref() {
        Some(profile) if profile.secret_ref.is_some() => {
            let reference = profile
                .secret_ref
                .as_deref()
                .context("proxy secret missing")?;
            let password = scheduler.secrets.get(reference).await?;
            let username = profile
                .username
                .clone()
                .context("proxy password requires a username")?;
            Some(ProxyCredentials { username, password })
        }
        _ => None,
    };
    let client = scheduler
        .clients
        .get_or_create(ClientContext {
            key: ClientKey {
                proxy_profile_id,
                account_id: config.account_id,
                cookie_ref: config.cookie_ref,
                auth_profile_id: client_wide.map(|(id, _)| id),
                auth_revision: client_wide.map_or(0, |(_, revision)| revision),
                replay_scope: replay_scope.as_ref().map(|scope| scope.key()),
                tls_revision: defaults.tls_revision,
                address_policy,
            },
            proxy: config.proxy,
            proxy_credentials,
            cookie_jar,
            custom_ca_pem: defaults.custom_ca_pem,
            auth: auth_material,
            replay_scope,
        })
        .await?;
    Ok(NetworkClient {
        client,
        headers,
        profile_boundary,
        provider_credential,
    })
}

/// Which stored credential a transfer may carry for this account.
///
/// Almost always the account's own secret, which for an OAuth provider *is* the access token.
/// The exception is a provider whose person registered their own application (RD-106-03): there
/// the account's secret is the **client** secret, every renewal still needs it, and the access
/// token lives beside the sign-in flow. Handing the first to [`provider_authorization`] would
/// put the client secret in an `Authorization` header on every transfer — the wrong credential,
/// sent where the right one belongs.
///
/// Real-Debrid is the other provider of that shape and never noticed, because its download
/// addresses are generated and carry no bearer at all. Box is the first whose bytes come from
/// the API host itself (RD-120-05), which is where this became reachable.
///
/// An account whose sign-in has not produced a token yet gets `None` rather than a fallback:
/// the transfer then goes out unauthenticated and the provider says so, which is a legible
/// failure. The fallback would be the client secret, and that is not.
async fn provider_transfer_credential(
    scheduler: &SchedulerHandle,
    config: &rd_db::NetworkClientConfig,
) -> Result<Option<ProviderCredential>> {
    let Some(provider) = config.account_provider.clone() else {
        return Ok(None);
    };
    let username = config.account_username.clone();
    if !rd_plugin_host::provider_token_beside_the_flow(&provider) {
        return Ok(config
            .account_secret_ref
            .clone()
            .map(|reference| ProviderCredential {
                provider,
                username,
                reference,
            }));
    }
    let Some(account_id) = config.account_id else {
        return Ok(None);
    };
    let stored = scheduler
        .database
        .auth_flow(account_id)
        .await?
        .and_then(|flow| flow.access_ref);
    Ok(stored.map(|reference| ProviderCredential {
        provider,
        username,
        reference,
    }))
}

/// The `Authorization` header the account's own credential puts on a request to `target`.
///
/// Two shapes, one gate (`rd_plugin_host::provider_download_authorization`): an OAuth-signed
/// provider's access token as `Bearer` (RD-106-04), and `Basic` for a provider whose row
/// declares `transfer_auth = "basic"` (RD-120-38) — Seedr's file addresses and Pixeldrain's.
/// Either way only over TLS and only to an exact host the provider's own manifest listed under
/// `secret_domains`. `target` is the address the request goes to, never the one the download
/// started from: a resolver answering with somebody else's host, or a source redirecting to
/// one, must not take the credential there.
///
/// The secret is read from the vault only once the gate has said yes. The inner `Err` is an
/// account that cannot form a Basic pair — a provider that requires a user name and an account
/// without one — and becomes the download's failure; it names no part of the credential.
pub(super) async fn provider_authorization(
    scheduler: &SchedulerHandle,
    credential: Option<&ProviderCredential>,
    target: &url::Url,
) -> Result<std::result::Result<Option<(String, String)>, Failure>> {
    let Some(credential) = credential else {
        return Ok(Ok(None));
    };
    // The gate first: a host outside `secret_domains` does not even open the vault.
    if !rd_plugin_host::provider_download_carries_credential(&credential.provider, target) {
        return Ok(Ok(None));
    }
    let secret = scheduler.secrets.get(&credential.reference).await?;
    Ok(rd_plugin_host::provider_download_authorization(
        &credential.provider,
        target,
        credential.username.as_deref(),
        secret.expose_secret(),
    )
    .map(|value| value.map(|value| ("authorization".to_owned(), value))))
}

/// Renders the `Authorization` value for a profile.
fn authorization_value(profile: &rd_core::AuthProfile, secret: &SecretString) -> Result<String> {
    match profile.method {
        AuthMethod::Bearer => Ok(format!("Bearer {}", secret.expose_secret())),
        AuthMethod::Basic => {
            let username = profile
                .username
                .as_deref()
                .context("basic auth profile has no username")?;
            let encoded =
                BASE64_STANDARD.encode(format!("{username}:{}", secret.expose_secret()).as_bytes());
            Ok(format!("Basic {encoded}"))
        }
        AuthMethod::Cookies => anyhow::bail!("cookie profiles carry no authorization header"),
    }
}
