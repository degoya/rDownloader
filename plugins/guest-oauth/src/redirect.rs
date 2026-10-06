//! The sign-in every OAuth plugin with an RFC 6749 token endpoint runs, once (RD-1120-10, PL-1).
//!
//! Box, Dropbox, Google Drive, OneDrive and the example plugin ran the same five calls, apart
//! from what a [`Provider`] states: the endpoints, the scope, the slug its codes go under, the
//! name its messages carry, the extra parameters its authorization server wants, whether it
//! offers a device flow and whether its client is public (PKCE) or confidential (a secret on
//! every grant). A plugin declares its `Provider` and ends in
//! [`redirect_plugin!`](crate::redirect_plugin). pCloud and Put.io are not in it: they read
//! their answer differently (RD-1110-04) and share only [`no_device_flow`].
//!
//! Three things the host guarantees shape every function here:
//!
//! - **No credential is ever in a guest.** Tokens go back through `store-oauth-token`, and a
//!   later request names one only as `{{secret:<reference>}}`, which the host expands on the
//!   way out.
//! - **The address the person is sent to is checked.** It must be on a domain the plugin's
//!   manifest declares, or the host refuses it.
//! - **Nothing is remembered between calls.** What `begin` has to hand to `poll` travels in
//!   `flow-state`, which the host stores verbatim and never shows.

use plugin_common::{device_flow::sanitize_error, pkce};

use crate::http::{self, RequestQuery};
use crate::token::{self, TokenAnswer, Waiting};
use crate::types::{Failure, FailureKind};
use crate::{
    AuthorizationRequest, DeviceAuthorization, TokenOutcome, accept_json, credentials, form, host,
    refuse, retry_after, unguessable_value,
};

/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be. Every provider here
/// accepts plain `http` for the loopback address, and each wants this exact one registered.
pub const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";

/// What the host replaces with this installation's own registered client (RD-106-04, rule 8).
/// Spelled here rather than imported because a guest links nothing of the host's.
pub const CLIENT_ID_MARKER: &str = "{{client_id}}";

/// How long a started flow may sit open. A provider's authorization code may live much
/// shorter (Box: thirty seconds); the walk through its pages is what this bounds.
const FLOW_SECONDS: u64 = 600;

/// The wait a rate-limited exchange gets when the provider named none.
const DEFAULT_WAIT_SECONDS: u64 = 30;

/// RFC 8628's grant type for a device poll.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// How the token endpoint knows the exchange is the flow that started.
#[derive(Clone, Copy, Debug)]
pub enum Client {
    /// PKCE: the verifier travels from `begin` to `poll` in `flow-state`, and the exchange
    /// carries it together with the redirect URI.
    Public,
    /// No PKCE, a client secret on every grant (Box). The secret is the account's own,
    /// registered by the person, and travels only as the marker of `secret_reference`.
    Confidential {
        /// The vault slot the person filled with their application's secret.
        secret_reference: &'static str,
        /// The `client_not_configured` message when the account has no such secret.
        unregistered: &'static str,
    },
}

/// Whether the provider offers the device entrance (RD-106-01).
#[derive(Clone, Copy, Debug)]
pub enum Device {
    /// Offered: where a device sign-in asks for the code the person types.
    Endpoint(&'static str),
    /// Not offered, and `device-begin`/`device-poll` answer with a stable refusal rather than a
    /// trap: the world requires them, and the manifest's `oauth_flows = ["redirect"]` means the
    /// host never calls them. The value names what is signed in through the browser.
    BrowserOnly(&'static str),
}

/// Everything an OAuth plugin states about its provider; the flow itself is this module's.
#[derive(Clone, Copy)]
pub struct Provider {
    /// The plugin's slug, the prefix of every translation code it reports (`dropbox_oauth`).
    pub slug: &'static str,
    /// Who refuses or answers in a message (`dropbox`, `microsoft`, `the provider`).
    pub name: &'static str,
    /// Where the person agrees. On a domain `manifest.toml` declares.
    pub authorize_endpoint: &'static str,
    /// Where the code, the device code and the refresh material are exchanged for tokens.
    pub token_endpoint: &'static str,
    /// Put into the authorization URL verbatim and into every request: usually
    /// [`CLIENT_ID_MARKER`], which the host expands.
    pub client_id: &'static str,
    /// The scope the authorization URL and a device sign-in ask for; `None` leaves the
    /// parameter out, and the provider takes the application's own configuration.
    pub scope: Option<&'static str>,
    /// Appended to the authorization URL as it stands, each pair starting with `&`.
    pub authorize_extra: &'static str,
    /// Appended to the code exchange and the renewal, never to a device poll.
    pub token_extra: &'static [(&'static str, &'static str)],
    pub client: Client,
    pub device: Device,
    /// What the provider's token answer says "wait" with, besides HTTP 429.
    pub waiting: Waiting,
    /// The plugin's mapping of a provider `error` to the code its catalogue carries.
    pub refusal_code: fn(&str) -> &'static str,
}

/// The template the host expands into the vault value named `reference`.
#[must_use]
pub fn secret_template(reference: &str) -> String {
    format!("{{{{secret:{reference}}}}}")
}

/// The refusal both device functions answer with when the provider offers no device flow.
#[must_use]
pub fn no_device_flow(slug: &str, product: &str) -> Failure {
    refuse(
        slug,
        "flow_unsupported",
        format!("{product} is signed in through the browser, not with a device code"),
        FailureKind::Unsupported,
    )
}

impl Provider {
    fn refuse(&self, code: &str, message: impl Into<String>, category: FailureKind) -> Failure {
        refuse(self.slug, code, message, category)
    }

    /// The address the person is sent to; with a verifier, the PKCE challenge is in it.
    #[must_use]
    pub fn authorization_url(&self, state: &str, verifier: Option<&str>) -> String {
        // The client id is left as it is: the marker is the host's to substitute, because a
        // guest has no way to read a configured value.
        let mut url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}",
            self.authorize_endpoint,
            self.client_id,
            pkce::percent_encode(REDIRECT_URI),
        );
        if let Some(scope) = self.scope {
            url.push_str(&format!("&scope={}", pkce::percent_encode(scope)));
        }
        url.push_str(&format!("&state={}", pkce::percent_encode(state)));
        if let Some(verifier) = verifier {
            url.push_str(&format!(
                "&code_challenge={}&code_challenge_method=S256",
                pkce::percent_encode(&pkce::challenge(verifier))
            ));
        }
        url.push_str(self.authorize_extra);
        url
    }

    /// Refuses before any request when a confidential client's account carries no registered
    /// application.
    ///
    /// `secret-available` reports whether a credential exists and never what it is. Asking it
    /// first turns a puzzle into an instruction: without it the provider would answer
    /// `invalid_client`, and the person would go looking for a fault in a registration they
    /// never made.
    fn require_registered_application(&self, account_id: &str) -> Result<(), Failure> {
        match self.client {
            Client::Confidential {
                secret_reference,
                unregistered,
            } if !host::secret_available(account_id, secret_reference) => Err(self.refuse(
                "client_not_configured",
                unregistered,
                FailureKind::AuthRequired,
            )),
            _ => Ok(()),
        }
    }

    /// `begin`: builds the address to send the person to.
    ///
    /// # Errors
    ///
    /// `client_not_configured` for a confidential client without its secret, `no_entropy` when
    /// the host supplied no randomness.
    pub fn begin(&self, account_id: &str) -> Result<AuthorizationRequest, Failure> {
        self.require_registered_application(account_id)?;
        // Both values come from the host's random source and from nothing else. A value
        // computed from the account or the time of day is a value anybody can recompute.
        let verifier = match self.client {
            Client::Public => Some(unguessable_value(self.slug)?),
            Client::Confidential { .. } => None,
        };
        let state = unguessable_value(self.slug)?;
        Ok(AuthorizationRequest {
            authorization_url: self.authorization_url(&state, verifier.as_deref()),
            // The host compares it with what the callback quotes; a callback naming a value no
            // flow claims matches nothing and is dropped.
            state,
            expires_in_seconds: Some(FLOW_SECONDS),
            // The verifier, never the challenge: the half the provider has not seen. A
            // confidential client has nothing to carry.
            flow_state: verifier,
        })
    }

    /// `poll`: exchanges the code the callback carried.
    ///
    /// # Errors
    ///
    /// A provider that could not be reached, which ends nothing: the host tries again.
    pub fn poll(
        &self,
        account_id: &str,
        code: &str,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        let fields = match self.client {
            Client::Public => {
                // Without the verifier there is nothing to prove the exchange with; failing
                // says so once instead of sending a request the provider is bound to reject.
                let Some(verifier) = flow_state.filter(|value| !value.is_empty()) else {
                    return Ok(TokenOutcome::Failed(self.refuse(
                        "code_expired",
                        "the sign-in has no verifier to continue with",
                        FailureKind::AuthRequired,
                    )));
                };
                form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", self.client_id),
                    ("code", code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", REDIRECT_URI),
                ])
            }
            Client::Confidential {
                secret_reference, ..
            } => {
                self.require_registered_application(account_id)?;
                form(&[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("client_id", self.client_id),
                    ("client_secret", &secret_template(secret_reference)),
                ])
            }
        };
        self.redeem(account_id, self.grant(fields))
    }

    /// `refresh`: mints a new access token from the stored refresh material, which
    /// `credential_ref` names and the host substitutes on the way out.
    ///
    /// # Errors
    ///
    /// `client_not_configured` for a confidential client without its secret; a provider that
    /// could not be reached.
    pub fn refresh(
        &self,
        account_id: &str,
        credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        self.require_registered_application(account_id)?;
        let Some(reference) = credential_ref.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(self.refuse(
                "refresh_refused",
                "this account has no stored sign-in to renew",
                FailureKind::AuthRequired,
            )));
        };
        let refresh_token = secret_template(&reference);
        let fields = match self.client {
            Client::Public => form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id),
                ("refresh_token", &refresh_token),
            ]),
            Client::Confidential {
                secret_reference, ..
            } => form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
                ("client_id", self.client_id),
                ("client_secret", &secret_template(secret_reference)),
            ]),
        };
        self.redeem(account_id, self.grant(fields))
    }

    /// `device-begin`: asks for a device code and reports what the person has to be shown.
    ///
    /// No PKCE, and that is not an omission: a device flow has no authorization code and no
    /// redirect to intercept. What binds the exchange is the device code, which travels in
    /// `flow-state` and is never shown to anybody.
    ///
    /// # Errors
    ///
    /// `flow_unsupported` without a device flow, `bad_reply` for an answer that is not a device
    /// code, and a provider that could not be reached.
    pub fn device_begin(&self) -> Result<DeviceAuthorization, Failure> {
        let endpoint = match self.device {
            Device::Endpoint(endpoint) => endpoint,
            Device::BrowserOnly(product) => return Err(no_device_flow(self.slug, product)),
        };
        let mut fields = vec![("client_id", self.client_id)];
        if let Some(scope) = self.scope {
            fields.push(("scope", scope));
        }
        let response = http::http_request("POST", endpoint, &form(&fields), &accept_json(), &[])?;
        let body = String::from_utf8_lossy(&response.body);
        let Some(code) = token::read_device_code(&body) else {
            return Err(self.refuse(
                "bad_reply",
                format!(
                    "{} answered {} to the device sign-in request",
                    self.name, response.status
                ),
                FailureKind::Permanent,
            ));
        };
        Ok(DeviceAuthorization {
            // As the provider gave it; the host refuses an address outside the manifest's
            // domains.
            verification_url: code.verification_url,
            user_code: Some(code.user_code),
            expires_in_seconds: code.expires_in,
            interval_seconds: code.interval,
            flow_state: Some(code.device_code),
        })
    }

    /// `device-poll`: asks whether the person has confirmed yet. `authorization_pending` comes
    /// back as `Pending` through the same reader as the code exchange.
    ///
    /// # Errors
    ///
    /// `flow_unsupported` without a device flow; a provider that could not be reached.
    pub fn device_poll(
        &self,
        account_id: &str,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        if let Device::BrowserOnly(product) = self.device {
            return Err(no_device_flow(self.slug, product));
        }
        let Some(device_code) = flow_state.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(self.refuse(
                "code_expired",
                "the sign-in has no device code to continue with",
                FailureKind::AuthRequired,
            )));
        };
        let fields = form(&[
            ("grant_type", DEVICE_GRANT),
            ("client_id", self.client_id),
            ("device_code", &device_code),
        ]);
        self.redeem(account_id, fields)
    }

    /// A code exchange's or a renewal's fields with the provider's own extras appended.
    fn grant(&self, mut fields: Vec<RequestQuery>) -> Vec<RequestQuery> {
        fields.extend(form(self.token_extra));
        if matches!(self.client, Client::Confidential { .. }) {
            // A confidential client's form never carried an empty field (Box's own `form`).
            fields.retain(|field| !field.value_template.is_empty());
        }
        fields
    }

    /// Sends `fields` to the token endpoint and turns the answer into an outcome.
    fn redeem(&self, account_id: &str, fields: Vec<RequestQuery>) -> Result<TokenOutcome, Failure> {
        let response =
            http::http_request("POST", self.token_endpoint, &fields, &accept_json(), &[])?;
        self.outcome(
            account_id,
            response.status,
            retry_after(&response.headers).as_deref(),
            &String::from_utf8_lossy(&response.body),
        )
    }

    /// Turns a token endpoint's answer into the outcome the host acts on.
    ///
    /// A provider that could not be reached never arrives here: `http-request` fails, the guest
    /// returns that failure, and the host keeps the stored token and tries again later. Only a
    /// refusal the provider actually made becomes `failed`, because confusing the two would
    /// sign people out every time their connection dropped.
    ///
    /// # Errors
    ///
    /// The host refused to store a granted token.
    pub fn outcome(
        &self,
        account_id: &str,
        status: u16,
        header: Option<&str>,
        body: &str,
    ) -> Result<TokenOutcome, Failure> {
        match token::read_token_answer(&self.waiting, status, header, body) {
            TokenAnswer::Granted {
                access_token,
                refresh_token,
                expires_in_seconds,
            } => {
                // Stored before `Authorized` is returned: the host takes that answer to mean
                // the credential is already kept. From this call on, both values are redacted
                // out of anything this plugin logs.
                credentials::store_oauth_token(
                    account_id,
                    &access_token,
                    refresh_token.as_deref(),
                    expires_in_seconds,
                )?;
                Ok(TokenOutcome::Authorized)
            }
            // The provider's `error_description` is never read: a sentence written for a
            // developer, which no shape check makes safe.
            TokenAnswer::Refused(error) => Ok(TokenOutcome::Failed(self.refuse(
                (self.refusal_code)(&error),
                format!(
                    "{} refused the sign-in: {}",
                    self.name,
                    sanitize_error(&error)
                ),
                FailureKind::AuthRequired,
            ))),
            // A rate limit, or a device code not confirmed yet, says nothing about the
            // credential, so it is a wait and not a failure.
            TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(
                seconds.unwrap_or(DEFAULT_WAIT_SECONDS),
            )),
            TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(self.refuse(
                "bad_reply",
                format!("{} answered {status} to the token request", self.name),
                FailureKind::Permanent,
            ))),
        }
    }
}

#[cfg(test)]
#[path = "redirect_tests.rs"]
mod tests;
