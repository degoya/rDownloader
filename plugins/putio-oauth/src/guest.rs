//! The component: Put.io's authorization code exchange, and the renewal that would outlive it.
//!
//! Redirect only. Put.io runs an out-of-band entrance for devices without a browser, but this
//! project could not find it stated in a published specification, so `device-begin` and
//! `device-poll` answer with a stable code rather than a trap — the world requires them to
//! exist, and the manifest's `oauth_flows = ["redirect"]` means the host never calls them.
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

/// The vault reference the client secret is kept under. Sent as a template and never as a
/// value; the host expands it towards `api.put.io` and nowhere else.
const CLIENT_SECRET_REFERENCE: &str = "putio_client_secret";

/// Where the person agrees. On a domain `manifest.toml` declares, which is what stops a signed
/// plugin sending somebody to a sign-in page of its own choosing.
const AUTHORIZE_ENDPOINT: &str = "https://api.put.io/v2/oauth2/authenticate";
/// Where the code is exchanged for a token.
const TOKEN_ENDPOINT: &str = "https://api.put.io/v2/oauth2/access_token";
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";

/// How long the person has to finish the sign-in before the flow is written off. Ten minutes,
/// the figure the other redirect plugins in this tree use.
const AUTHORIZATION_SECONDS: u64 = 600;

/// The wait a rate-limited exchange gets when Put.io named none.
const DEFAULT_WAIT_SECONDS: u64 = 30;

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("putio_oauth.{code}")),
        params: Vec::new(),
    }
}

/// A value nobody can recompute: the host's random bytes, base64url-encoded.
///
/// An empty answer means the host refused, and a sign-in is failed rather than continued with
/// a value the plugin made up. The `state` is the only thing standing between this flow and a
/// callback somebody else wrote, so a guessable one is no protection at all.
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

/// The `Retry-After` Put.io sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// Turns the token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "Put.io could not be reached". That never arrives as an
/// answer — `http-request` fails instead, the guest returns that failure with `?`, and the host
/// keeps the stored token and tries again later. Only a refusal Put.io actually made becomes
/// `failed`, because a plugin that confused the two would sign people out of their account
/// every time their connection dropped.
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
            // the value is redacted out of anything this plugin logs.
            credentials::store_oauth_token(
                account_id,
                &access_token,
                refresh_token.as_deref(),
                expires_in_seconds,
            )?;
            Ok(TokenOutcome::Authorized)
        }
        flow::TokenAnswer::Refused(error) => Ok(TokenOutcome::Failed(refuse(
            flow::refusal_code(&error),
            format!("put.io refused the sign-in: {error}"),
            FailureKind::AuthRequired,
        ))),
        // A rate limit says nothing about the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(
            seconds.unwrap_or(DEFAULT_WAIT_SECONDS),
        )),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("put.io answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

/// The one refusal both device functions answer with.
fn no_device_flow() -> Failure {
    refuse(
        "flow_unsupported",
        "put.io is signed in through the browser, not with a device code".to_owned(),
        FailureKind::Unsupported,
    )
}

impl Guest for Component {
    /// Builds the address to send the person to.
    ///
    /// No `code_challenge`: Put.io's exchange authenticates with the client secret and
    /// publishes no PKCE support, and a proof nobody checks is decoration. `state` is the
    /// protection that is real here, and it comes from the host's random source and from
    /// nothing else — neither the account nor the time of day takes part, because a value
    /// computed from those is a value anybody holding them can recompute.
    fn begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<AuthorizationRequest, Failure> {
        let state = unguessable_value()?;
        let authorization_url = format!(
            "{AUTHORIZE_ENDPOINT}?client_id={}&response_type=code&redirect_uri={}&state={}",
            // Left as the marker: the host substitutes this installation's own client id
            // here, because a guest has no way to read a configured value.
            CLIENT_ID_MARKER,
            pkce::percent_encode(REDIRECT_URI),
            pkce::percent_encode(&state),
        );
        Ok(AuthorizationRequest {
            authorization_url,
            // The host stores this and compares it with what the callback quotes. A callback
            // naming a value no flow claims matches nothing and is dropped.
            state,
            expires_in_seconds: Some(AUTHORIZATION_SECONDS),
            // Nothing to carry. Without PKCE there is no verifier, and a value stored here
            // that no later call reads would be state the host keeps for no reason.
            flow_state: None,
        })
    }

    /// Exchanges the code the callback carried.
    ///
    /// A `GET` with the parameters in the query, which is the shape Put.io's token endpoint
    /// documents. The client secret travels as `{{secret:putio_client_secret}}` and is
    /// expanded by the host on the way out, so it is never in this guest's memory — and
    /// because the host expands it towards `api.put.io` alone, an endpoint constant somebody
    /// edited to point elsewhere would get the marker and not the value.
    fn poll(
        account_id: String,
        code: String,
        _flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        let response = http::http_request(
            "GET",
            TOKEN_ENDPOINT,
            &form(&[
                ("client_id", CLIENT_ID_MARKER),
                (
                    "client_secret",
                    &format!("{{{{secret:{CLIENT_SECRET_REFERENCE}}}}}"),
                ),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT_URI),
                ("code", &code),
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

    /// Not offered. The manifest says so, so the host never calls this; being asked anyway is
    /// a refusal with a stable code rather than a trap.
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

    /// Mints a new access token from stored refresh material.
    ///
    /// Put.io issues none and states no expiry, so the host's renewal sweep — which reads only
    /// flows carrying both — never reaches this. It is written as the ordinary exchange all
    /// the same: the day Put.io starts issuing refresh material, the renewal that already
    /// exists works, and until then an account with nothing to renew is told so once instead
    /// of being retried for ever.
    ///
    /// The refresh token is never in this function either: `credential_ref` names it and the
    /// host substitutes the value into `{{secret:<reference>}}` on the way out.
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
            "GET",
            TOKEN_ENDPOINT,
            &form(&[
                ("client_id", CLIENT_ID_MARKER),
                (
                    "client_secret",
                    &format!("{{{{secret:{CLIENT_SECRET_REFERENCE}}}}}"),
                ),
                ("grant_type", "refresh_token"),
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
