//! The component: Microsoft's authorization code with PKCE, its device code, and the renewal
//! that outlives both.
//!
//! Both entrances, because Microsoft offers both and the manifest says so
//! (`oauth_flows = ["redirect", "device"]`). Whichever one a sign-in takes, it ends in the same
//! `store-oauth-token` call with the same refresh material, and `refresh` renews it the same
//! way.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
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

/// What the host replaces with this installation's own application (client) id. Spelled here
/// rather than imported because a guest links nothing of the host's.
const CLIENT_ID_MARKER: &str = "{{client_id}}";

/// Where the person agrees. On a domain `manifest.toml` declares, which is what stops a signed
/// plugin sending somebody to a sign-in page of its own choosing. The `common` tenant lets a
/// personal Microsoft account and a work or school account sign in through one address — the
/// application has to be registered for both, which the account's hint says.
const AUTHORIZE_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
/// Where a device sign-in asks for the code the person types.
const DEVICE_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
/// Where the code, the device code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
/// There is deliberately no client id in this file (RD-106-04, rule 8). A client id compiled
/// in here would sit in the git history of a public repository, in every signed `.rdplug` and
/// in every release artifact — and Microsoft's throttling is counted per application, so one
/// compiled-in registration would have every installation in the world sharing one budget.
/// Registered per installation, each one has its own, in its own tenant, revocable by its own
/// administrator. The account's username field holds it, and the host expands the marker.
///
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be. Microsoft accepts an
/// `http` loopback address for a public client, and this exact one has to be entered under
/// the application's *Mobile and desktop applications* platform.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";
/// The least this needs, and it is worth being exact about why it is not less. `Files.Read`
/// covers the account's own drive, but the whole reason this plugin exists — a sharing link,
/// which is also the only way into SharePoint content here — goes through `/shares/{id}`,
/// which Graph grants to `Files.Read.All` and not to `Files.Read`. Still read-only: it cannot
/// delete, cannot share and cannot write, which matters more than usual because the same
/// token is spent by two other plugins. `offline_access` is what makes a refresh token exist at
/// all; without it the person would be asked again every hour.
const SCOPE: &str = "Files.Read.All offline_access";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("onedrive_oauth.{code}")),
        params: Vec::new(),
    }
}

/// A value nobody can recompute: the host's random bytes, base64url-encoded.
///
/// Asked for once per value rather than split from a single draw — the verifier and the `state`
/// must not be two halves of one secret. An empty answer means the host refused, and a sign-in
/// is failed rather than continued with a value the plugin made up.
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

/// The `Retry-After` Microsoft sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// Turns a token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "Microsoft could not be reached". That never arrives as
/// an answer — `http-request` fails instead, the guest returns that failure with `?`, and the
/// host keeps the stored token and tries again later. Only a refusal Microsoft actually made
/// becomes `failed`, because a plugin that confused the two would sign people out of their
/// OneDrive every time their connection dropped.
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
            // credential is already kept, so saying it first would be a lie. From this call on,
            // both values are redacted out of anything this plugin logs.
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
                    "microsoft refused the sign-in: {}",
                    flow::sanitize_error(&error)
                ),
                FailureKind::AuthRequired,
            )))
        }
        // A rate limit, or a device code the person has not confirmed yet, says nothing about
        // the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds.unwrap_or(30))),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("microsoft answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

impl Guest for Component {
    /// Builds the address to send the person to, with the challenge already in it.
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
             &state={}&code_challenge={}&code_challenge_method=S256&response_mode=query",
            // Left as the marker: the host substitutes this installation's own application
            // id here, because a guest has no way to read a configured value.
            CLIENT_ID_MARKER,
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
            // The verifier, never the challenge: this is the half Microsoft has not seen, and
            // sending it with the code is what proves the exchange is the same flow.
            flow_state: Some(verifier),
        })
    }

    /// Exchanges the code the callback carried, using the bookkeeping `begin` handed over.
    fn poll(
        account_id: String,
        code: String,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        // Without the verifier there is nothing to prove the exchange with. Failing says so once
        // instead of sending a request Microsoft is bound to reject.
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
                ("client_id", CLIENT_ID_MARKER),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", REDIRECT_URI),
                ("scope", SCOPE),
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

    /// Asks Microsoft for a device code and reports what the person has to be shown.
    ///
    /// No redirect, so no `state` to echo and no callback to wait for: what has to survive to
    /// the poll is the device code, and it travels in `flow_state` exactly as the PKCE
    /// verifier does above. There is no PKCE here, and that is not an omission: PKCE binds an
    /// authorization code to the client that asked for it, and a device flow has no
    /// authorization code and no redirect to intercept — what binds this exchange is the
    /// device code itself, which Microsoft issued to this client and nobody else ever sees.
    fn device_begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<DeviceAuthorization, Failure> {
        let response = http::http_request(
            "POST",
            DEVICE_ENDPOINT,
            &form(&[("client_id", CLIENT_ID_MARKER), ("scope", SCOPE)]),
            &accept_json(),
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        let Some(code) = flow::read_device_code(&body) else {
            return Err(refuse(
                "bad_reply",
                format!(
                    "microsoft answered {} to the device sign-in request",
                    response.status
                ),
                FailureKind::Permanent,
            ));
        };
        Ok(DeviceAuthorization {
            // As Microsoft gave it — `https://microsoft.com/devicelogin`. The host refuses an
            // address outside the domains this manifest declares, which is what stops a
            // plugin sending somebody to a sign-in page of its own choosing.
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
        // Without the device code there is nothing to poll with. Failing says so once instead
        // of asking Microsoft a question it cannot answer, for ever.
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
                ("client_id", CLIENT_ID_MARKER),
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
    /// substitutes the value into `{{secret:<reference>}}` on the way out. This is what a
    /// OneDrive account lives on — Microsoft's access tokens last about an hour, so without
    /// this every download started more than an hour after a sign-in would ask somebody to
    /// sign in again. The scope is repeated because Microsoft's token endpoint asks for it on
    /// a refresh, and answers a narrower token when it is left out.
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
                ("client_id", CLIENT_ID_MARKER),
                ("refresh_token", &format!("{{{{secret:{reference}}}}}")),
                ("scope", SCOPE),
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
