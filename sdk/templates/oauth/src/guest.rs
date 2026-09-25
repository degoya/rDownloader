//! The component: authorization code with PKCE, and the renewal that outlives it.
//!
//! Replace the four constants below with the provider's own endpoints and the client you
//! registered with it. Everything else is the shape of the exchange rather than the provider,
//! and changes far less often.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "oauth-plugin",
});

use exports::rdownloader::plugin::oauth::{
    AuthorizationRequest, DeviceAuthorization, Guest, TokenOutcome,
};
use rdownloader::plugin::{
    credentials, host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{flow, pkce};

/// Where the person agrees. Must be on a domain `manifest.toml` declares.
const AUTHORIZE_ENDPOINT: &str = "https://api.example.com/oauth/authorize";
/// Where a device sign-in asks for the code the person types (RD-106-01). A provider that
/// offers no device flow has no such endpoint; delete this and the two `device-*` functions'
/// bodies below, replace them with a `flow_unsupported` refusal, and drop `"device"` from
/// `oauth_flows` in `manifest.toml` — the host then never calls them at all.
const DEVICE_ENDPOINT: &str = "https://api.example.com/oauth/device/code";
/// Where the code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://api.example.com/oauth/token";
/// The client you registered with the provider. A client id identifies the application, not
/// the person, so it is public by design. A client *secret* is not: a provider that requires
/// one takes it the way `refresh` below takes the refresh token — as the template
/// `{{secret:<reference>}}` built from `credential_ref`, which the host expands on the way out
/// and hands back to nobody.
const CLIENT_ID: &str = "replace-with-your-registered-client-id";
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used. This is the application's own callback; adjust the origin
/// to the address your users reach rDownloader on and register exactly that with the provider.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";
/// Ask for the least the plugin needs, and for the renewal scope — without it there is no
/// refresh material and the person is asked again every time the token ages out.
const SCOPE: &str = "offline_access";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

/// A value nobody can recompute: the host's random bytes, base64url-encoded.
///
/// Asked for once per value rather than split from a single draw — the verifier and the `state`
/// must not be two halves of one secret, which is exactly what the clock-derived pair this
/// replaced amounted to. An empty answer means the host refused, and a sign-in is failed rather
/// than continued with a value the plugin made up.
fn unguessable_value() -> Result<String, Failure> {
    let random = host::random_bytes(pkce::VERIFIER_BYTES as u32);
    pkce::verifier(&random).ok_or_else(|| {
        refuse(
            "no_entropy",
            "the host did not supply the randomness this sign-in needs".to_owned(),
            FailureKind::Permanent,
        )
    })
}

fn form(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect()
}

fn accept_json() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "application/json".to_owned(),
    }]
}

/// The `Retry-After` a provider sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// Turns a token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "the provider could not be reached". That never
/// arrives as an answer — `http-request` fails instead, the guest returns that failure with
/// `?`, and the host keeps the stored token and tries again later. Only a refusal the
/// provider actually made becomes `failed`.
fn outcome(
    account_id: &str,
    status: u16,
    header: Option<&str>,
    body: &str,
) -> Result<TokenOutcome, Failure> {
    match flow::read_token_answer(status, header, body) {
        flow::TokenAnswer::Granted {
            access_token,
            refresh_token,
            expires_in_seconds,
        } => {
            // Stored before `Authorized` is returned: the host takes that answer to mean the
            // credential is already kept, so saying it first would be a lie. From this call
            // on, both values are redacted out of anything this plugin logs.
            credentials::store_oauth_token(
                account_id,
                &access_token,
                refresh_token.as_deref(),
                expires_in_seconds,
            )?;
            Ok(TokenOutcome::Authorized)
        }
        flow::TokenAnswer::Refused(error) => {
            let code = flow::refusal_code(&error);
            Ok(TokenOutcome::Failed(refuse(
                code,
                format!(
                    "the provider refused the sign-in: {}",
                    flow::sanitize_error(&error)
                ),
                FailureKind::AuthRequired,
            )))
        }
        // A rate limit says nothing about the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds.unwrap_or(30))),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("the provider answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

impl Guest for Component {
    /// Builds the address to send the person to, with the challenge already in it.
    ///
    /// `credential_ref` names the person's own registered client, when the provider needs one
    /// in this step. This scaffold's does not, so it is unused here rather than absent. The
    /// account id is unused for a second reason: what the flow is tied to is the `state` the
    /// host stores next to it, never a value computed from the account.
    fn begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<AuthorizationRequest, Failure> {
        // Both values come from the host's random source and from nothing else. Neither the
        // account nor the time of day takes part: a value computed from those is a value
        // anybody holding them can recompute, and a recomputable verifier is no verifier.
        let verifier = unguessable_value()?;
        let state = unguessable_value()?;
        let authorization_url = format!(
            "{AUTHORIZE_ENDPOINT}?response_type=code&client_id={}&redirect_uri={}&scope={}\
             &state={}&code_challenge={}&code_challenge_method=S256",
            pkce::percent_encode(CLIENT_ID),
            pkce::percent_encode(REDIRECT_URI),
            pkce::percent_encode(SCOPE),
            pkce::percent_encode(&state),
            pkce::percent_encode(&pkce::challenge(&verifier)),
        );
        Ok(AuthorizationRequest {
            authorization_url,
            // The host stores this and compares it with what the callback quotes. A callback
            // naming a value no flow claims matches nothing and is dropped.
            state,
            expires_in_seconds: Some(600),
            // The verifier, never the challenge: this is the half the provider has not seen,
            // and sending it with the code is what proves the exchange is the same flow.
            flow_state: Some(verifier),
        })
    }

    /// Exchanges the code the callback carried, using the bookkeeping `begin` handed over.
    fn poll(
        account_id: String,
        code: String,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        // Without the verifier there is nothing to prove the exchange with. Failing says so
        // once instead of sending a request the provider is bound to reject.
        let Some(verifier) = flow_state.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "code_expired",
                "the sign-in has no verifier to continue with".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", REDIRECT_URI),
            ]),
            &accept_json(),
            &[],
        )?;
        outcome(
            &account_id,
            response.status,
            retry_after(&response.headers).as_deref(),
            &String::from_utf8_lossy(&response.body),
        )
    }

    /// Asks the provider for a device code and reports what the person has to be shown.
    ///
    /// The device entrance (RD-106-01). No redirect, so no `state` to echo and no callback to
    /// wait for: what has to survive to the poll is the device code, and it travels in
    /// `flow_state` exactly as the PKCE verifier does above.
    ///
    /// There is no PKCE here, and that is not an omission. PKCE binds an authorization code
    /// to the client that asked for it, and a device flow has no authorization code and no
    /// redirect to intercept — what binds this exchange is the device code itself, which the
    /// provider issued to this client and nobody else ever sees.
    fn device_begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<DeviceAuthorization, Failure> {
        let response = http::http_request(
            "POST",
            DEVICE_ENDPOINT,
            &form(&[("client_id", CLIENT_ID), ("scope", SCOPE)]),
            &accept_json(),
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        let Some(code) = flow::read_device_code(&body) else {
            return Err(refuse(
                "bad_reply",
                format!(
                    "the provider answered {} to the device sign-in request",
                    response.status
                ),
                FailureKind::Permanent,
            ));
        };
        Ok(DeviceAuthorization {
            // As the provider gave it. The host refuses an address outside the domains this
            // manifest declares, which is what stops a plugin sending somebody to a sign-in
            // page of its own choosing.
            verification_url: code.verification_url,
            user_code: Some(code.user_code),
            expires_in_seconds: code.expires_in,
            interval_seconds: code.interval,
            // The device code, never the user code: this is what the poll is made with, and
            // it is never shown to anybody.
            flow_state: Some(code.device_code),
        })
    }

    /// Asks whether the person has confirmed yet, with the device code handed back.
    fn device_poll(
        account_id: String,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        // Without the device code there is nothing to poll with. Failing says so once
        // instead of asking the provider a question it cannot answer, for ever.
        let Some(device_code) = flow_state.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "code_expired",
                "the sign-in has no device code to continue with".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", &device_code),
            ]),
            &accept_json(),
            &[],
        )?;
        // The same reader as the redirect exchange, because it is the same endpoint and the
        // same document. `authorization_pending` comes back as `Pending`, so the host waits
        // instead of ending a sign-in the person is still in the middle of.
        outcome(
            &account_id,
            response.status,
            retry_after(&response.headers).as_deref(),
            &String::from_utf8_lossy(&response.body),
        )
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// The refresh token is never in this function: `credential_ref` names it, and the host
    /// substitutes the value into `{{secret:<reference>}}` on the way out.
    fn refresh(
        account_id: String,
        credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        let Some(reference) = credential_ref.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "refresh_refused",
                "this account has no stored sign-in to renew".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", &format!("{{{{secret:{reference}}}}}")),
            ]),
            &accept_json(),
            &[],
        )?;
        outcome(
            &account_id,
            response.status,
            retry_after(&response.headers).as_deref(),
            &String::from_utf8_lossy(&response.body),
        )
    }
}

export!(Component);
