//! `ResolverHost` for `NativeHost`: what a plugin's host calls do -- requests, cookies, waits,
//! captchas, secrets, key derivation and the tokens a sign-in stores.
//!
//! Split out of `host.rs` (PLUG-21).

use std::time::Duration;

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_http::{CookieScope, import_cookie_jar};
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolverHost,
};
use reqwest::{Method, cookie::CookieStore};
use secrecy::ExposeSecret;
use url::Url;

use super::super::expand::{
    allowed_header, cookie_scope, expand_request, expand_url, method_allowed,
    reference_active_for_account, validate_request_domain,
};
use super::{
    EXCHANGE_TIMEOUT, Limits, MAX_SINGLE_WAIT, NO_AUTH_PROFILE, NativeHost, SEND_TIMEOUT,
    address_policy, check_reach, exchange, response_limit, token_expiry, upload_allowance,
};

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
        let policy = address_policy(&self.own, &request.url);
        let (client, proxied) = self.client(identity, &request.url, &policy).await?;
        check_reach(&policy, &request.url, proxied, &self.own).await?;
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
        // Sending the body is not the server being slow to answer: its time comes on top of both
        // limits rather than out of them (RA-HOST-04).
        let upload = upload_allowance(request.body.len());
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        exchange(
            builder,
            &request.url,
            carries_credential,
            response_limit(),
            Limits {
                head: SEND_TIMEOUT + upload,
                whole: EXCHANGE_TIMEOUT + upload,
            },
            &self.own,
        )
        .await
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
        // A part a sign-in keeps beside its token is asked of its own row (RD-150-09): the
        // personal client secret exists once the device flow handed it out, whether or not the
        // token exchange after it has finished.
        if rd_provider_registry::by_slug(&provider)
            .is_some_and(|spec| spec.flow_part_slot(reference).is_some())
        {
            return self
                .database
                .auth_flow_part(account_id, reference)
                .await
                .is_ok_and(|part| part.is_some());
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
        let expires_at =
            expires_in_seconds.map(|seconds| token_expiry(chrono::Utc::now(), seconds));
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

    /// Stores one named part of a sign-in beside its token (RD-150-09).
    ///
    /// The name has to be a part slot of the account's own provider, live in the account's
    /// mode: a plugin cannot invent a place to write to, cannot overwrite the token or what the
    /// person typed through this call, and an account holding a pasted key keeps no sign-in
    /// parts at all. The new value is referenced before the one it replaces is dropped, for the
    /// reason `store_token` gives.
    async fn store_flow_secret(
        &self,
        account_id: AccountId,
        name: &str,
        value: &str,
    ) -> Result<(), Failure> {
        if value.trim().is_empty() {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.store_token_empty",
                "An authentication flow returned an empty credential",
            ));
        }
        let (provider, mode) = super::account_credentials(&self.database, account_id).await?;
        let is_part = rd_provider_registry::by_slug(&provider)
            .is_some_and(|spec| spec.flow_part_slot(name).is_some());
        if !is_part || !reference_active_for_account(&provider, name, mode) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.store_token_not_allowed",
                "This provider keeps no sign-in part of that name",
            ));
        }
        let stored = self
            .secrets
            .put_string(value.to_owned())
            .await
            .map_err(super::permanent)?;
        match self
            .database
            .set_auth_flow_part(account_id, name.to_owned(), stored.clone())
            .await
        {
            Ok(replaced) => {
                if let Some(old) = replaced {
                    let _ = self.secrets.remove(&old).await;
                }
                Ok(())
            }
            Err(error) => {
                // Nothing references the value just written, so it is ours to take back.
                let _ = self.secrets.remove(&stored).await;
                Err(super::permanent(error))
            }
        }
    }
}
