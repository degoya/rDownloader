//! The `NativeHost` side of one request before it is sent: which credentials, user name and
//! client id it may carry, and the HTTP client it goes out on.
//!
//! Split out of `host.rs` (PLUG-21).

use std::sync::Arc;

use rd_core::{AccountId, Failure, FailureKind};
use rd_http::{ClientContext, ClientKey, CookieScope, ProxyCredentials, import_cookie_jar};
use rd_plugin_api::{ClientIdentity, HostHttpRequest};
use reqwest::cookie::Jar;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use super::super::expand::{
    account_missing, client_not_configured, cookie_scope, has_bare_username_marker,
    has_client_id_marker, has_granted_secret_marker, has_username_marker,
    reference_active_for_account, secret_domain_allowed, secret_target_not_allowed,
    username_domain_allowed,
};
use super::super::references::{Secrets, secret_references};
use super::{NO_AUTH_PROFILE, NativeHost};

impl NativeHost {
    /// Every credential the request names, each loaded on its own (RD-120-39).
    ///
    /// Each distinct reference goes through [`Self::named_secret`], the same gate one reference
    /// alone passes, and the first that fails refuses the whole request -- before anything is
    /// sent. The cap on how many one request may name is checked before any of them is loaded.
    pub(in crate::native) async fn request_secrets(
        &self,
        identity: &ClientIdentity,
        request: &HostHttpRequest,
    ) -> Result<Secrets, Failure> {
        let references = secret_references(request)?;
        let mut secrets = Secrets::default();
        // The reference-less form: an invocation that was granted one secret expands that one.
        // There is no account behind it and no reference for the plugin to aim at, so what
        // could go wrong here is limited to what was handed over. A named reference beside it
        // is not filled with it: that one passes the account gate below, like any other.
        if has_granted_secret_marker(request) {
            let Some(granted) = request.granted_secret.as_deref() else {
                return Err(secret_target_not_allowed());
            };
            secrets.set_granted(
                self.secrets
                    .get(granted)
                    .await
                    .map_err(super::vault_failure)?,
            );
        }
        for reference in references {
            let value = self.named_secret(identity, request, reference).await?;
            secrets.insert(reference, value);
        }
        Ok(secrets)
    }

    /// The value of one named reference, if this request may carry it to this address.
    async fn named_secret(
        &self,
        identity: &ClientIdentity,
        request: &HostHttpRequest,
        reference: &str,
    ) -> Result<SecretString, Failure> {
        let account_id = identity.account_id.ok_or_else(account_missing)?;
        let (provider, mode) = super::account_credentials(&self.database, account_id).await?;
        // The access token of a provider that keeps it beside the flow (RD-106-03).
        //
        // Its slot is declared like any other, so `reference_active_for_account` and the domain
        // gate below apply unchanged; what differs is only where the value is read from. The
        // account's own secret is the client secret the person registered, and answering with
        // that here would send the wrong credential to the provider on every request.
        let filled_by_flow = rd_provider_registry::by_slug(&provider).is_some_and(|spec| {
            spec.secret_slot(reference)
                .is_some_and(rd_provider_registry::SecretSlot::is_filled_by_flow)
        });
        if filled_by_flow {
            // The mode gate as well, since RD-150-09 put flow slots beside a typed one: an
            // account holding a pasted API key must not reach what a sign-in would have kept.
            if !reference_active_for_account(&provider, reference, mode)
                || !secret_domain_allowed(reference, &request.url)
            {
                return Err(secret_target_not_allowed());
            }
            // A named part is read from its own row, the token from the flow's.
            let is_part = rd_provider_registry::by_slug(&provider)
                .is_some_and(|spec| spec.flow_part_slot(reference).is_some());
            let stored = if is_part {
                self.database
                    .auth_flow_part(account_id, reference)
                    .await
                    .map_err(super::permanent)?
            } else {
                self.database
                    .auth_flow(account_id)
                    .await
                    .map_err(super::permanent)?
                    .and_then(|flow| flow.access_ref)
            };
            let stored = stored.ok_or_else(|| {
                Failure::coded(
                    FailureKind::AuthRequired,
                    "plugin.provider_secret_missing",
                    "Provider secret is missing",
                )
            })?;
            return self
                .secrets
                .get(&stored)
                .await
                .map_err(super::vault_failure);
        }
        // The renewal material of this account's own sign-in (RD-106-03).
        //
        // It is a vault reference rather than one of the provider's declared slots, because
        // the host minted it in `store_oauth_token` and handed it back to the plugin as
        // `credential-ref`. Without this branch the two checks below refuse it -- and even a
        // relaxed check would then expand the *account's* secret, which is the access token
        // and not the material a renewal is made with. So an OAuth plugin could never reach
        // what the contract says it was given, and every renewal failed with
        // `plugin.secret_target_not_allowed` instead.
        //
        // Nothing is widened by it: the reference has to be exactly the one stored on this
        // account's flow row, and the address has to be one the provider's own credential may
        // reach anyway.
        if let Some(renewal) = self.renewal_reference(account_id, reference).await? {
            if !username_domain_allowed(&provider, mode, &request.url) {
                return Err(secret_target_not_allowed());
            }
            return self
                .secrets
                .get(&renewal)
                .await
                .map_err(super::vault_failure);
        }
        if !reference_active_for_account(&provider, reference, mode)
            || !secret_domain_allowed(reference, &request.url)
        {
            return Err(secret_target_not_allowed());
        }
        let config = self
            .database
            .network_client_config(
                Some(account_id),
                identity.proxy_profile_id,
                None,
                NO_AUTH_PROFILE,
                &request.url,
            )
            .await
            .map_err(super::permanent)?;
        let stored = config.account_secret_ref.ok_or_else(|| {
            Failure::coded(
                FailureKind::AuthRequired,
                "plugin.provider_secret_missing",
                "Provider secret is missing",
            )
        })?;
        self.secrets
            .get(&stored)
            .await
            .map_err(super::vault_failure)
    }

    /// This account's stored renewal reference, when `reference` is exactly it.
    ///
    /// Deliberately an equality test against what the host itself wrote, not a shape test: a
    /// plugin that guessed at vault references must find nothing, and one that was handed its
    /// own account's must find that and nothing else.
    async fn renewal_reference(
        &self,
        account_id: AccountId,
        reference: &str,
    ) -> Result<Option<String>, Failure> {
        let stored = self
            .database
            .auth_flow(account_id)
            .await
            .map_err(super::permanent)?
            .and_then(|flow| flow.refresh_ref);
        Ok(stored.filter(|stored| stored == reference))
    }

    /// `{{client_id}}` resolves to the OAuth client this installation registered for itself,
    /// which is stored as the account's username (RD-106-04).
    ///
    /// Two gates, and only two, because a client id is not a credential: the account's provider
    /// has to be one that is signed in with OAuth, and the request has to be going somewhere the
    /// plugin's own manifest allows — which the sandbox has already decided by the time this
    /// runs. Deliberately **not** the secret-domain gate `{{username}}` uses: that one pins a
    /// credential to the hosts it may reach, and a client id has no such hosts. A provider's
    /// authorization endpoint and its token endpoint are usually two different names, and both
    /// are published.
    ///
    /// A provider of any other credential kind gets `None`, so the marker refuses rather than
    /// quietly handing a password field's neighbour to a stranger.
    pub(in crate::native) async fn request_client_id(
        &self,
        identity: &ClientIdentity,
        request: &HostHttpRequest,
    ) -> Result<Option<String>, Failure> {
        if !has_client_id_marker(request) {
            return Ok(None);
        }
        let account_id = identity.account_id.ok_or_else(account_missing)?;
        let (provider, _) = super::account_credentials(&self.database, account_id).await?;
        let is_oauth = rd_provider_registry::by_slug(&provider)
            .is_some_and(|spec| spec.credentials == rd_provider_registry::CredentialKind::OAuth);
        if !is_oauth {
            return Err(secret_target_not_allowed());
        }
        let client_id = super::account_username(&self.database, account_id)
            .await?
            .filter(|value| !value.trim().is_empty());
        client_id.map(Some).ok_or_else(client_not_configured)
    }

    /// `{{username}}` resolves to the account's stored username; it is credential material,
    /// so it is only expanded where the provider's secret may also be sent (same domain gate).
    ///
    /// The second half of the answer is whether the provider row lets the name be empty
    /// (`username_required = false`, RD-120-38). That allowance covers the `{{basic:…}}` pair
    /// alone -- Pixeldrain's API key is a Basic password under an empty name -- so a request
    /// that also carries a bare `{{username}}` marker is refused exactly as before.
    pub(in crate::native) async fn request_username(
        &self,
        identity: &ClientIdentity,
        request: &HostHttpRequest,
    ) -> Result<(Option<String>, bool), Failure> {
        if !has_username_marker(request) {
            return Ok((None, false));
        }
        let account_id = identity.account_id.ok_or_else(account_missing)?;
        let (provider, mode) = super::account_credentials(&self.database, account_id).await?;
        if !username_domain_allowed(&provider, mode, &request.url) {
            return Err(secret_target_not_allowed());
        }
        let optional = !has_bare_username_marker(request)
            && rd_provider_registry::by_slug(&provider).is_some_and(|spec| !spec.username_required);
        let username = super::account_username(&self.database, account_id)
            .await?
            .filter(|value| !value.is_empty());
        match username {
            Some(username) => Ok((Some(username), optional)),
            None if optional => Ok((None, true)),
            None => Err(Failure::coded(
                FailureKind::AuthRequired,
                "plugin.username_missing",
                "Account username is missing",
            )),
        }
    }

    /// The client for one request, and whether it goes through a proxy.
    ///
    /// Built with `policy` as its address rule (RA-HOST-01): without a proxy it resolves names
    /// through the guarded resolver, and every redirect hop to a literal address is judged.
    pub(super) async fn client(
        &self,
        identity: &ClientIdentity,
        scope: &Url,
        policy: &rd_http::AddressPolicy,
    ) -> Result<(reqwest::Client, bool), Failure> {
        let defaults = self.network_defaults.read().await.clone();
        let config = self
            .database
            .network_client_config(
                identity.account_id,
                identity.proxy_profile_id,
                defaults.global_proxy_profile_id,
                NO_AUTH_PROFILE,
                scope,
            )
            .await
            .map_err(super::permanent)?;
        let cookie_jar = match &config.cookie_ref {
            Some(reference) => {
                let content = self
                    .secrets
                    .get(reference)
                    .await
                    .map_err(super::permanent)?;
                let scope = cookie_scope(config.account_provider.as_deref(), scope);
                CookieScope::provider(&scope)
                    .and_then(|scope| import_cookie_jar(content.expose_secret(), &scope))
                    .map_err(super::permanent)?
            }
            None => Arc::new(Jar::default()),
        };
        let proxy_id = config.proxy.as_ref().map(|proxy| proxy.id);
        let proxied = config.proxy.is_some();
        let proxy_credentials = match config.proxy.as_ref() {
            Some(proxy) if proxy.secret_ref.is_some() => {
                let stored = proxy.secret_ref.as_deref().ok_or_else(|| {
                    Failure::coded(
                        FailureKind::AuthRequired,
                        "plugin.proxy_secret_missing",
                        "Proxy secret is missing",
                    )
                })?;
                let password = self
                    .secrets
                    .get(stored)
                    .await
                    .map_err(super::vault_failure)?;
                let username = proxy.username.clone().ok_or_else(|| {
                    Failure::coded(
                        FailureKind::Permanent,
                        "plugin.proxy_user_missing",
                        "Proxy username is missing",
                    )
                })?;
                Some(ProxyCredentials { username, password })
            }
            _ => None,
        };
        self.clients
            .get_or_create(ClientContext {
                key: ClientKey {
                    proxy_profile_id: proxy_id,
                    account_id: identity.account_id,
                    cookie_ref: config.cookie_ref,
                    // See NO_AUTH_PROFILE: resolver clients never carry a domain profile.
                    auth_profile_id: None,
                    auth_revision: 0,
                    replay_scope: None,
                    tls_revision: defaults.tls_revision,
                    // A plugin's requests are bounded by its manifest's domains, and the
                    // addresses those resolve to by the rule (RA-HOST-01).
                    address_policy: Some(policy.clone()),
                },
                proxy: config.proxy,
                proxy_credentials,
                cookie_jar,
                custom_ca_pem: defaults.custom_ca_pem,
                auth: None,
                // Resolver requests are contained by the manifest's request domains
                // (`validate_redirect`), not by a replay's consented origin set.
                replay_scope: None,
            })
            .await
            .map(|client| (client, proxied))
            .map_err(super::permanent)
    }
}
