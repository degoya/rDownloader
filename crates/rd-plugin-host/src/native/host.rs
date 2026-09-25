use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
/// Resolver plugins authenticate through provider accounts, which carry their own scoped
/// cookie jar and secrets. Domain auth profiles deliberately do not apply here; they are a
/// transfer-time credential, so a profile shows up on the download but not during resolve.
const NO_AUTH_PROFILE: rd_core::AuthProfileSelection = rd_core::AuthProfileSelection::None;

use rd_core::{AccountId, Failure, FailureKind};
use rd_http::{
    ClientContext, ClientKey, ClientPool, CookieScope, ProxyCredentials, SharedNetworkDefaults,
    import_cookie_jar,
};
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolver, ClientIdentity, HostHttpRequest,
    HostHttpResponse, ResolvedHeader, ResolverHost,
};
use reqwest::{
    Method,
    cookie::{CookieStore, Jar},
};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use super::expand::{
    account_missing, allowed_header, client_not_configured, cookie_scope, expand_request,
    expand_url, has_bare_username_marker, has_client_id_marker, has_granted_secret_marker,
    has_username_marker, http_failure, method_allowed, reference_active_for_account,
    secret_domain_allowed, secret_target_not_allowed, transient, username_domain_allowed,
    validate_redirect, validate_request_domain,
};
use super::references::{Secrets, secret_references};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Longest single countdown a resolver may ask for. Real free-download timers are under two
/// minutes; anything beyond this is a parse error or a limit that belongs in `ip-blocked`.
const MAX_SINGLE_WAIT: Duration = Duration::from_secs(10 * 60);

pub(super) struct NativeHost {
    pub(super) database: rd_db::Database,
    clients: ClientPool,
    pub(super) secrets: rd_secrets::SecretStore,
    network_defaults: SharedNetworkDefaults,
    captcha: Option<Arc<dyn CaptchaSolver>>,
}

impl NativeHost {
    pub(super) fn new(
        database: rd_db::Database,
        clients: ClientPool,
        secrets: rd_secrets::SecretStore,
        network_defaults: SharedNetworkDefaults,
        captcha: Option<Arc<dyn CaptchaSolver>>,
    ) -> Self {
        Self {
            database,
            clients,
            secrets,
            network_defaults,
            captcha,
        }
    }
}

#[async_trait]
impl ResolverHost for NativeHost {
    async fn http_request(
        &self,
        identity: &ClientIdentity,
        mut request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        // A resolver is measured against the provider registry as well; an extension type
        // serves no provider, so the registry's union says nothing about where its service
        // lives and its own manifest — already applied above it — is the whole answer.
        if request.authority == rd_plugin_api::RequestAuthority::Provider {
            validate_request_domain(&request.url)?;
        }
        let secrets = self.request_secrets(identity, &request).await?;
        let (username, username_optional) = self.request_username(identity, &request).await?;
        let client_id = self.request_client_id(identity, &request).await?;
        let carries_credential = expand_request(
            &mut request,
            &secrets,
            username.as_deref(),
            client_id.as_deref(),
            username_optional,
        )?;
        // The address itself may carry the granted secret, for a webhook whose token is part
        // of its path. Already past the domain gate above; `expand_url` checks that it is
        // still the same host afterwards.
        request.url = expand_url(&request.url, secrets.granted())?;
        for query in request.query.drain(..) {
            request
                .url
                .query_pairs_mut()
                .append_pair(&query.name, &query.value_template);
        }
        let client = self.client(identity, &request.url).await?;
        let method = Method::from_bytes(request.method.as_bytes()).map_err(super::permanent)?;
        // Reading is the default, and `PROPFIND` reads: it lists a collection. A storage
        // destination also writes — that is what it is for — and WebDAV spells that `PUT`,
        // `MKCOL` and `DELETE`; nothing else may reach those.
        if !method_allowed(method.as_str(), request.write_methods) {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.http_method_not_allowed",
                "Plugin HTTP method is not allowed",
            ));
        }
        // A method that carries content states its length even when there is none: hyper sends
        // an empty HTTP/1.1 body with no `Content-Length` at all, and a server may answer that
        // with 411. The plugin cannot say it itself — the header is not on the allowlist, and
        // must not be, since it would have to agree with the body the host sends (RD-120-60).
        let empty_content = request.body.is_empty() && matches!(method.as_str(), "POST" | "PUT");
        let mut builder = client.request(method, request.url.clone());
        if empty_content {
            builder = builder.header(reqwest::header::CONTENT_LENGTH, "0");
        }
        for header in request.headers {
            if !allowed_header(&header.name) {
                return Err(Failure::coded(
                    FailureKind::Permanent,
                    "plugin.http_header_not_allowed",
                    "Resolver HTTP header is not allowed",
                ));
            }
            builder = builder.header(header.name, header.value_template);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let mut response = tokio::time::timeout(Duration::from_secs(15), builder.send())
            .await
            .map_err(|_| transient("plugin.http_timeout", "Resolver HTTP request timed out"))?
            .map_err(http_failure)?;
        let status = response.status().as_u16();
        let final_url = response.url().clone();
        validate_redirect(&request.url, &final_url, carries_credential)?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value.to_str().ok().map(|value| ResolvedHeader {
                    name: name.as_str().to_owned(),
                    value: value.to_owned(),
                })
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_failure)? {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(Failure::coded(
                    FailureKind::Permanent,
                    "plugin.response_too_large",
                    "Resolver response exceeds the size limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HostHttpResponse {
            status,
            final_url,
            headers,
            body,
        })
    }

    async fn cookies_get(&self, account_id: AccountId, url: &Url) -> Vec<(String, String)> {
        let Ok(config) = self
            .database
            .network_client_config(Some(account_id), None, None, NO_AUTH_PROFILE, url)
            .await
        else {
            return Vec::new();
        };
        let Some(reference) = config.cookie_ref else {
            return Vec::new();
        };
        let Ok(content) = self.secrets.get(&reference).await else {
            return Vec::new();
        };
        let scope = cookie_scope(config.account_provider.as_deref(), url);
        let imported = CookieScope::provider(&scope)
            .and_then(|scope| import_cookie_jar(content.expose_secret(), &scope));
        let Ok(jar) = imported else {
            return Vec::new();
        };
        jar.cookies(url)
            .and_then(|header| header.to_str().ok().map(str::to_owned))
            .map(|header| {
                header
                    .split(';')
                    .filter_map(|pair| pair.trim().split_once('='))
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Waits out a hoster countdown. Dropping the resolve future (pause, cancel, shutdown)
    /// cancels the sleep, so a waiting download stops as promptly as a transferring one.
    async fn wait(&self, _client: &ClientIdentity, seconds: u32) -> Result<(), Failure> {
        let requested = Duration::from_secs(u64::from(seconds));
        if requested > MAX_SINGLE_WAIT {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.wait_too_long",
                "Resolver requested an implausibly long wait",
            ));
        }
        tokio::time::sleep(requested).await;
        Ok(())
    }

    async fn captcha_allowance(&self) -> Duration {
        match &self.captcha {
            Some(captcha) => captcha.allowance().await,
            None => rd_plugin_api::DEFAULT_CAPTCHA_ALLOWANCE,
        }
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        challenge: CaptchaChallenge,
        time_limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        let Some(captcha) = &self.captcha else {
            return Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            ));
        };
        captcha.solve(challenge, time_limit).await
    }

    async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool {
        let Ok((provider, mode)) = super::account_credentials(&self.database, account_id).await
        else {
            return false;
        };
        // Answering per reference is also how a resolver learns which mode its account is in:
        // DDownload probes `ddownload_password` against `ddownload_api_key` and takes the
        // branch the host admits, without the mode ever crossing the sandbox boundary.
        if !reference_active_for_account(&provider, reference, mode) {
            return false;
        }
        // A slot a flow fills is asked of the flow (RD-106-03). Answering it from the account's
        // credential would say "signed in" for an account that has only registered its
        // application -- and a resolver that believed it would send the client secret as a
        // Bearer token to the provider.
        let filled_by_flow = rd_provider_registry::by_slug(&provider).is_some_and(|spec| {
            spec.secret_slot(reference)
                .is_some_and(rd_provider_registry::SecretSlot::is_filled_by_flow)
        });
        if filled_by_flow {
            return self
                .database
                .auth_flow(account_id)
                .await
                .is_ok_and(|flow| flow.is_some_and(|flow| flow.access_ref.is_some()));
        }
        self.database
            .account_secret_refs(account_id)
            .await
            .is_ok_and(|refs| refs.is_some_and(|(secret, _)| secret.is_some()))
    }

    /// Runs a derivation over the credential behind `reference` (RD-120-20).
    ///
    /// The mirror image of [`Self::request_secrets`]: there the host reads a credential and
    /// puts it into a request the plugin described, here it reads one and puts it through a
    /// computation the plugin described. The plaintext never leaves the host. Which value is
    /// read, and which first step it admits, depends on where it came from -- a person or a
    /// sign-in (RD-120-30); both are decided in `native::signin`.
    async fn derive_from_secret(
        &self,
        client: &ClientIdentity,
        reference: &str,
        steps: &[rd_plugin_api::DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        self.derive_over(client, reference, steps).await
    }

    /// Stores what an authentication flow produced.
    ///
    /// The caller never names a reference: this looks up which one the account's provider
    /// owns and refuses if that provider has none, so a plugin cannot write into a slot that
    /// is not its own. The new value is written before the old one is dropped, because an
    /// interruption between the two is survivable in that order and loses the credential in
    /// the other.
    async fn store_token(&self, account_id: AccountId, value: &str) -> Result<(), Failure> {
        if value.trim().is_empty() {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.store_token_empty",
                "An authentication flow returned an empty credential",
            ));
        }
        let account = self
            .database
            .list_accounts()
            .await
            .map_err(super::permanent)?
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| {
                Failure::coded(
                    FailureKind::AccountInvalid,
                    "plugin.account_unavailable",
                    "Account is not available",
                )
            })?;
        // A provider whose slots say a sign-in fills one keeps the session there, beside what
        // the person typed (RD-120-30). Writing it over the password -- what this did for
        // MEGA until then -- left the next sign-in nothing to compute over.
        if rd_provider_registry::by_slug(&account.provider)
            .is_some_and(|spec| spec.flow_secret_slot().is_some())
        {
            return self.store_session(account_id, value).await;
        }
        if rd_provider_registry::by_slug(&account.provider)
            .and_then(|spec| spec.secret_reference().map(str::to_owned))
            .is_none()
        {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.store_token_not_allowed",
                "This provider stores no credential of its own",
            ));
        }
        let (old_secret, cookie_ref) = self
            .database
            .account_secret_refs(account_id)
            .await
            .map_err(super::permanent)?
            .unwrap_or((None, None));
        let new_secret = self
            .secrets
            .put_string(value.to_owned())
            .await
            .map_err(super::permanent)?;
        let update = rd_db::UpdateAccount {
            provider: account.provider,
            label: account.label,
            username: account.username,
            credential_mode: account.credential_mode,
            secret_ref: Some(new_secret.clone()),
            cookie_ref,
            proxy_profile_id: account.proxy_profile_id,
            enabled: account.enabled,
        };
        if let Err(error) = self.database.update_account(account_id, update).await {
            // The account still points at the old value, so the new one is unreferenced.
            let _ = self.secrets.remove(&new_secret).await;
            return Err(super::permanent(error));
        }
        if let Some(old) = old_secret.filter(|old| old != &new_secret) {
            let _ = self.secrets.remove(&old).await;
        }
        Ok(())
    }

    /// Stores what an OAuth exchange produced, access token and renewal material alike.
    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        // Where the access token goes depends on whether the account's own credential is
        // already taken (RD-106-03). For almost every provider it is not: the token *is* the
        // account's credential, and it takes the ordinary path, so everything guarding
        // `store_token` -- the provider having to own a reference, the old value dropped only
        // once the new one is in -- guards this too.
        //
        // The exception is a provider whose person registered their own application. Their
        // client secret is in `accounts.secret_ref` and the next renewal needs it, so writing
        // the token over it would break the very thing that keeps them signed in. Such a
        // provider declares a slot of its own for the token, and it lands beside the flow.
        let (provider, _) = super::account_credentials(&self.database, account_id).await?;
        let flow_slot = rd_provider_registry::by_slug(&provider)
            .and_then(|spec| spec.flow_secret_slot().map(|slot| slot.reference.clone()));
        let stored_access = match &flow_slot {
            None => {
                self.store_token(account_id, access_token).await?;
                None
            }
            Some(_) => Some(
                self.secrets
                    .put_string(access_token.to_owned())
                    .await
                    .map_err(super::permanent)?,
            ),
        };
        let previous_access = self
            .database
            .auth_flow(account_id)
            .await
            .map_err(super::permanent)?
            .and_then(|flow| flow.access_ref);
        let previous = self
            .database
            .auth_flow(account_id)
            .await
            .map_err(super::permanent)?
            .and_then(|flow| flow.refresh_ref);
        let refreshed = match refresh_token
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            Some(token) => Some(
                self.secrets
                    .put_string(token.to_owned())
                    .await
                    .map_err(super::permanent)?,
            ),
            // Not an error: plenty of providers hand over refresh material once, on the first
            // exchange, and expect it to keep working. Dropping it here would make the second
            // renewal impossible.
            None => previous.clone(),
        };
        let expires_at = expires_in_seconds
            .and_then(|seconds| i64::try_from(seconds).ok())
            .map(|seconds| chrono::Utc::now() + chrono::Duration::seconds(seconds));
        if let Err(error) = self
            .database
            .set_auth_flow_renewal(
                account_id,
                expires_at,
                refreshed.clone(),
                stored_access.clone(),
            )
            .await
        {
            // Nothing references the secrets we just wrote, so they are ours to take back.
            if refreshed != previous
                && let Some(orphan) = refreshed
            {
                let _ = self.secrets.remove(&orphan).await;
            }
            if let Some(orphan) = stored_access {
                let _ = self.secrets.remove(&orphan).await;
            }
            return Err(super::permanent(error));
        }
        if let Some(old) = previous.filter(|old| Some(old) != refreshed.as_ref()) {
            let _ = self.secrets.remove(&old).await;
        }
        // The token this replaced, in the same order and for the same reason: the new value is
        // referenced before the old one is dropped, so an interruption between the two leaves
        // a usable account rather than none.
        if let Some(old) = previous_access.filter(|old| Some(old) != stored_access.as_ref()) {
            let _ = self.secrets.remove(&old).await;
        }
        Ok(())
    }
}

impl NativeHost {
    /// Every credential the request names, each loaded on its own (RD-120-39).
    ///
    /// Each distinct reference goes through [`Self::named_secret`], the same gate one reference
    /// alone passes, and the first that fails refuses the whole request -- before anything is
    /// sent. The cap on how many one request may name is checked before any of them is loaded.
    pub(super) async fn request_secrets(
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
            secrets.set_granted(self.secrets.get(granted).await.map_err(super::permanent)?);
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
            if !secret_domain_allowed(reference, &request.url) {
                return Err(secret_target_not_allowed());
            }
            let stored = self
                .database
                .auth_flow(account_id)
                .await
                .map_err(super::permanent)?
                .and_then(|flow| flow.access_ref)
                .ok_or_else(|| {
                    Failure::coded(
                        FailureKind::AuthRequired,
                        "plugin.provider_secret_missing",
                        "Provider secret is missing",
                    )
                })?;
            return self.secrets.get(&stored).await.map_err(super::permanent);
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
            return self.secrets.get(&renewal).await.map_err(super::permanent);
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
        self.secrets.get(&stored).await.map_err(super::permanent)
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
    pub(super) async fn request_client_id(
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
    pub(super) async fn request_username(
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

    async fn client(
        &self,
        identity: &ClientIdentity,
        scope: &Url,
    ) -> Result<reqwest::Client, Failure> {
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
        let proxy_credentials = match config.proxy.as_ref() {
            Some(proxy) if proxy.secret_ref.is_some() => {
                let stored = proxy.secret_ref.as_deref().ok_or_else(|| {
                    Failure::coded(
                        FailureKind::AuthRequired,
                        "plugin.proxy_secret_missing",
                        "Proxy secret is missing",
                    )
                })?;
                let password = self.secrets.get(stored).await.map_err(super::permanent)?;
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
            .map_err(super::permanent)
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod host_tests;

#[cfg(test)]
#[path = "host_basic_tests.rs"]
mod host_basic_tests;
