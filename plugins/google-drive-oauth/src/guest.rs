//! The component: Google's authorization code with PKCE, and the renewal that outlives it.
//!
//! Redirect only. Google does run a device flow, but it is scoped to input-constrained devices
//! and does not grant the Drive scopes this needs, so `device-begin` and `device-poll` answer
//! with a stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway.
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

/// What the host replaces with this installation's own OAuth client. Spelled here rather than
/// imported because a guest links nothing of the host's.
const CLIENT_ID_MARKER: &str = "{{client_id}}";

/// Where the person agrees. On a domain `manifest.toml` declares, which is what stops a signed
/// plugin sending somebody to a sign-in page of its own choosing.
const AUTHORIZE_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Where the code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// The OAuth client **this installation** registered for itself, written as the marker the host
/// expands from the account (RD-106-04).
///
/// There is deliberately no client id in this file, and it is worth saying why rather than
/// leaving it to be rediscovered. A client id compiled in here would sit in the git history of
/// a public repository, in every signed `.rdplug` and in every release artifact — and, far more
/// practically, Google's quotas are counted per client, so one compiled-in project would have
/// every installation in the world sharing one allowance. Registered per installation, each one
/// has its own, and each one also decides its own client type rather than this project guessing
/// which type Google's token endpoint will accept.
///
/// A client id identifies the application to Google, not the person to the application: Google
/// publishes it in the address the person is sent to. So it is not a credential, it is stored
/// in the clear as the account's username, and the host will put it into the authorization URL
/// as well as into the two requests below. An account without one is refused with
/// `oauth.client_not_configured`, before anybody is sent anywhere.
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be. Google matches a
/// loopback redirect on everything but the port, so one registration serves every installation.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";
/// The least this needs: read what the account can see, and nothing else. `drive.readonly`
/// cannot delete, cannot share and cannot write — which matters more here than usual, because
/// the same token is spent by two other plugins.
const SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("google_drive_oauth.{code}")),
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

/// The `Retry-After` Google sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// Turns a token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "Google could not be reached". That never arrives as an
/// answer — `http-request` fails instead, the guest returns that failure with `?`, and the host
/// keeps the stored token and tries again later. Only a refusal Google actually made becomes
/// `failed`, because a plugin that confused the two would sign people out of their Drive every
/// time their connection dropped.
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
                    "google refused the sign-in: {}",
                    flow::sanitize_error(&error)
                ),
                FailureKind::AuthRequired,
            )))
        }
        // A rate limit says nothing about the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds.unwrap_or(30))),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("google answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

/// The one refusal both device functions answer with.
fn no_device_flow() -> Failure {
    refuse(
        "flow_unsupported",
        "google drive is signed in through the browser, not with a device code".to_owned(),
        FailureKind::Unsupported,
    )
}

impl Guest for Component {
    /// Builds the address to send the person to, with the challenge already in it.
    ///
    /// `access_type=offline` and `prompt=consent` are not decoration: without the first Google
    /// issues no refresh material at all, and without the second it issues it exactly once —
    /// so an account signed in twice would come back the second time with nothing to renew
    /// from, and the person would be asked again every hour for ever.
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
             &state={}&code_challenge={}&code_challenge_method=S256\
             &access_type=offline&prompt=consent&include_granted_scopes=true",
            // Left as the marker: the host substitutes this installation's own client id
            // here, because a guest has no way to read a configured value.
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
            // The verifier, never the challenge: this is the half Google has not seen, and
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
        // instead of sending a request Google is bound to reject.
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

    /// Not offered. The manifest says so, so the host never calls this; being asked anyway is a
    /// refusal with a stable code rather than a trap.
    fn device_begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<DeviceAuthorization, Failure> {
        Err(no_device_flow())
    }

    /// Not offered, for the same reason as `device-begin`.
    fn device_poll(
        _account_id: String,
        _flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        Err(no_device_flow())
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// The refresh token is never in this function: `credential_ref` names it, and the host
    /// substitutes the value into `{{secret:<reference>}}` on the way out. This is what a Drive
    /// account lives on — Google's access tokens last an hour, so without this every download
    /// started more than an hour after a sign-in would ask somebody to sign in again.
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
