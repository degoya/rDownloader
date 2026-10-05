//! The host side of the resolver world's imports -- `http`, `cookies`, `host` and `captcha` --
//! implemented on `PluginStoreState`, with the metered wait, captcha and request paths they
//! share with the extension worlds.
//!
//! Split out of `component.rs` (PLUG-21).

use std::{str::FromStr, sync::Arc, time::Instant};

use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{CaptchaAnswer, HostHttpRequest, HostRequestValue};
use url::Url;

use super::convert::{
    answer_shape_refused, from_wit_challenge, host_disconnected, permanent, to_wit_failure,
    unfollowed_redirect,
};
use super::{
    MIN_CAPTCHA_ALLOWANCE, random_bytes, wit_captcha, wit_cookies, wit_host, wit_http, wit_types,
};
use crate::{captcha_target_refused, domain_allowed, runtime::PluginStoreState};

impl wit_types::Host for PluginStoreState {}

impl wit_http::Host for PluginStoreState {
    async fn http_request(
        &mut self,
        method: String,
        url: String,
        query: Vec<wit_http::RequestQuery>,
        headers: Vec<wit_http::RequestHeader>,
        body: Vec<u8>,
    ) -> Result<wit_http::HttpResponse, wit_types::Failure> {
        self.controlled_http_request(method, url, query, headers, body)
            .await
            .map_err(to_wit_failure)
    }
}

impl wit_cookies::Host for PluginStoreState {
    async fn cookies_get(&mut self, account_id: String, url: String) -> Vec<(String, String)> {
        let Ok(account_id) = AccountId::from_str(&account_id) else {
            return Vec::new();
        };
        if self.identity().account_id != Some(account_id) {
            return Vec::new();
        }
        let Ok(url) = Url::parse(&url) else {
            return Vec::new();
        };
        if !domain_allowed(&url, self.allowed_domains()) {
            return Vec::new();
        }
        let Some(host) = self.host() else {
            return Vec::new();
        };
        let cookies = host.cookies_get(account_id, &url).await;
        self.remember_redactions(cookies.iter().map(|(_, value)| value.clone()));
        cookies
    }
}

impl wit_host::Host for PluginStoreState {
    async fn secret_available(&mut self, account_id: String, reference: String) -> bool {
        let Ok(account_id) = AccountId::from_str(&account_id) else {
            return false;
        };
        if self.identity().account_id != Some(account_id) {
            return false;
        }
        let Some(host) = self.host() else {
            return false;
        };
        host.secret_available(account_id, &reference).await
    }

    async fn now_unix_seconds(&mut self) -> u64 {
        match self.host() {
            Some(host) => host.now_unix_seconds(),
            None => u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default(),
        }
    }

    async fn random_bytes(&mut self, count: u32) -> Vec<u8> {
        random_bytes(count)
    }

    async fn wait(&mut self, seconds: u32) -> Result<(), wit_types::Failure> {
        self.controlled_wait(seconds).await.map_err(to_wit_failure)
    }

    async fn log(&mut self, level: String, message: String) {
        let message = self.redact_log(&message);
        match level.to_ascii_lowercase().as_str() {
            "error" => tracing::error!(plugin = true, "{message}"),
            "warn" => tracing::warn!(plugin = true, "{message}"),
            "debug" | "trace" => tracing::debug!(plugin = true, "{message}"),
            _ => tracing::info!(plugin = true, "{message}"),
        }
    }
}

impl wit_captcha::Host for PluginStoreState {
    async fn solve_captcha(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaSolution, wit_types::Failure> {
        self.solve_for_token(challenge)
            .await
            .map_err(to_wit_failure)
    }

    async fn solve_challenge(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaAnswer, wit_types::Failure> {
        let challenge = from_wit_challenge(challenge).map_err(to_wit_failure)?;
        let answer = self
            .controlled_solve_captcha(challenge)
            .await
            .map_err(to_wit_failure)?;
        Ok(match answer {
            CaptchaAnswer::Token(token) => wit_captcha::CaptchaAnswer::Token(token),
            CaptchaAnswer::Point(point) => {
                wit_captcha::CaptchaAnswer::Point(wit_captcha::ClickPoint {
                    x: point.x,
                    y: point.y,
                })
            }
        })
    }
}

impl PluginStoreState {
    /// Waits on the host's clock, bounded by the plugin's wait budget.
    async fn controlled_wait(&mut self, seconds: u32) -> Result<(), Failure> {
        let requested = std::time::Duration::from_secs(u64::from(seconds));
        let Some(granted) = self.claim_wait(requested) else {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.wait_budget_exhausted",
                "Plugin requested a longer wait than its budget allows",
            ));
        };
        let host = self.host().ok_or_else(host_disconnected)?;
        let identity = self.identity().clone();
        let seconds = u32::try_from(granted.as_secs()).unwrap_or(u32::MAX);
        host.wait(&identity, seconds).await
    }

    /// The token-shaped entry point. A click-point challenge is refused here, before any
    /// budget is claimed and before a service or a person is asked: `captcha-solution`
    /// cannot carry a point, so an answer would only be thrown away (RD-110-15).
    async fn solve_for_token(
        &mut self,
        challenge: wit_captcha::CaptchaChallenge,
    ) -> Result<wit_captcha::CaptchaSolution, Failure> {
        let challenge = from_wit_challenge(challenge)?;
        if challenge.answers_with_point() {
            return Err(answer_shape_refused());
        }
        match self.controlled_solve_captcha(challenge).await? {
            CaptchaAnswer::Token(token) => Ok(wit_captcha::CaptchaSolution { token }),
            CaptchaAnswer::Point(_) => Err(answer_shape_refused()),
        }
    }

    /// Solves a challenge the guest already had validated, within the waiting budget.
    async fn controlled_solve_captcha(
        &mut self,
        challenge: rd_plugin_api::CaptchaChallenge,
    ) -> Result<CaptchaAnswer, Failure> {
        let host = self.host().ok_or_else(host_disconnected)?;
        let identity = self.identity().clone();
        // A widget challenge names a page the browser extension will open, so it is an
        // outbound target like any other and is held to the same manifest boundary as
        // `net_http` (RD-107-03). Checked here rather than in the browser alone: the browser
        // is the second fence, and a plugin must not be able to name a page outside its
        // manifest even when none is attached.
        if let Some(page) = challenge.page_url() {
            let page = Url::parse(page).map_err(permanent)?;
            if !domain_allowed(&page, self.allowed_domains()) {
                return Err(captcha_target_refused());
            }
        }
        // Solving is a wait too: it runs on a service or a person, so it must not count
        // against the plugin's compute timeout. Reserve what answering may actually take —
        // a configured manual timeout can be far longer than one solver round trip — and
        // settle for the rest of the budget when that no longer fits.
        let wanted = host.captcha_allowance().await.min(self.wait_budget());
        let granted = (wanted >= MIN_CAPTCHA_ALLOWANCE)
            .then(|| self.claim_wait(wanted))
            .flatten();
        let Some(granted) = granted else {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.wait_budget_exhausted",
                "Plugin requested a captcha with no waiting budget left",
            ));
        };
        // The host answers within the time it was granted, so the resolver behind it stays
        // inside the execution deadline that reservation just bought.
        host.solve_captcha(&identity, challenge, granted).await
    }

    async fn controlled_http_request(
        &mut self,
        method: String,
        url: String,
        query: Vec<wit_http::RequestQuery>,
        headers: Vec<wit_http::RequestHeader>,
        body: Vec<u8>,
    ) -> Result<wit_http::HttpResponse, Failure> {
        // Only the `{{secret}}` the plugin wrote literally stays the granted marker; the same
        // text encoded into a path from somebody else's file name does not (RD-120-66).
        let url = crate::foreign_address::guest_url(&url).map_err(permanent)?;
        if !domain_allowed(&url, self.allowed_domains()) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.http_target_not_allowed",
                "Plugin HTTP target is not allowed by the manifest",
            ));
        }
        let host = self.host().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.host_disconnected",
                "Plugin host is not connected",
            )
        })?;
        let identity = self.identity().clone();
        let granted_secret = self.granted_secret().map(str::to_owned);
        let authority = self.request_authority();
        let write_methods = self.write_methods();
        // The execution deadline is wall-clock, so every second spent waiting for the hoster
        // used to be charged against the plugin's compute budget — with the HTTP timeout set to
        // the same 15s as that budget, one slow response was enough to end the call as
        // "plugin.timeout". Ten downloads at once made that routine. The waiting time is
        // credited back below, exactly as the transfer sink already does.
        let started = Instant::now();
        // Every redirect hop is put to the same list as the first address *before* it is
        // followed (RD-130-24). Checking only where the request ended let a `307` carry the
        // plugin's body to a host outside its domains first and be refused afterwards.
        let domains: Arc<[String]> = Arc::from(self.allowed_domains());
        let gate: rd_http::RedirectGate = Arc::new(move |hop: &Url| domain_allowed(hop, &domains));
        let request = host.http_request(
            &identity,
            HostHttpRequest {
                method,
                url,
                query: query
                    .into_iter()
                    .map(|value| HostRequestValue {
                        name: value.name,
                        value_template: value.value_template,
                    })
                    .collect(),
                headers: headers
                    .into_iter()
                    .map(|value| HostRequestValue {
                        name: value.name,
                        value_template: value.value_template,
                    })
                    .collect(),
                body,
                // From the store, never from the guest: a plugin writes `{{secret}}`
                // without naming a reference precisely because naming one is not its
                // business.
                granted_secret: granted_secret.clone(),
                authority,
                write_methods,
            },
        );
        // The answer is read up to what the manifest still allows, not a fixed cap (PLUG-06).
        let request =
            crate::native::with_response_allowance(self.remaining_response_bytes(), request);
        // Which addresses the request may reach is the store's to say, like the allowance: the
        // person's own network only where they supplied the address (RA-HOST-01).
        let request = crate::native::with_own_network(self.own_network(), request);
        let response = rd_http::with_redirect_gate(gate, request).await;
        // Credited on the failure path too: a request that timed out still spent that time
        // waiting on the network rather than computing.
        self.credit_host_time(started.elapsed());
        let response = response?;
        // A hop the gate refused comes back unfollowed, as the redirect itself; it is refused
        // here exactly like a request that had ended outside the list, so a plugin sees one
        // behaviour for "your redirect leaves your domains" whichever way it was caught.
        let outside = if domain_allowed(&response.final_url, self.allowed_domains()) {
            unfollowed_redirect(&response)
                .filter(|target| !domain_allowed(target, self.allowed_domains()))
        } else {
            Some(response.final_url.clone())
        };
        if let Some(outside) = outside {
            let redirect_host = outside.host_str().unwrap_or("(no host)");
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.redirect_outside_domains",
                format!("Plugin HTTP redirect to {redirect_host} leaves the manifest domains"),
            )
            .with_param("host", redirect_host));
        }
        self.account_response_bytes(response.body.len())
            .map_err(permanent)?;
        Ok(wit_http::HttpResponse {
            status: response.status,
            final_url: response.final_url.to_string(),
            headers: response
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect(),
            body: response.body,
        })
    }
}
