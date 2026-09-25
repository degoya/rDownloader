//! The component: Box's authorization code grant, and the renewal that outlives it.
//!
//! Redirect only. Box has no device flow, so `device-begin` and `device-poll` answer with a
//! stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway.
//!
//! And no PKCE. Box's authorization server accepts no `code_challenge`, and its token endpoint
//! requires a `client_secret` on every grant, so what proves an exchange is the same flow is
//! the secret the person registered rather than a verifier this plugin kept back. `state` is
//! still drawn from the host's random source and compared by the host, which is what keeps a
//! callback naming a flow nobody started from matching anything.
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

/// What the host replaces with this installation's own Box client id. Spelled here rather than
/// imported because a guest links nothing of the host's.
const CLIENT_ID_MARKER: &str = "{{client_id}}";

/// Where the person agrees. On a domain `manifest.toml` declares, which is what stops a signed
/// plugin sending somebody to a sign-in page of its own choosing.
const AUTHORIZE_ENDPOINT: &str = "https://account.box.com/api/oauth2/authorize";
/// Where the code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://api.box.com/oauth2/token";

/// The account's own credential: the client secret of the application the person registered.
///
/// Box is the one cloud drive of the four that leaves no choice here. Its OAuth 2.0 has no
/// public-client entrance and accepts no PKCE, so the token endpoint refuses every grant that
/// arrives without a secret. That is why the `box` provider carries two credential slots
/// (RD-106-03): this one, which the person fills and which every renewal still needs, and the
/// access token, which this plugin writes into a slot of its own.
const CLIENT_SECRET_REFERENCE: &str = "box_client_secret";

/// The application **this installation** registered for itself in the Box Developer Console.
///
/// There is deliberately no client id and no client secret in this file, and it is worth saying
/// why rather than leaving it to be rediscovered. A pair compiled in here would sit in the git
/// history of a public repository, in every signed `.rdplug` and in every release artefact —
/// not revocable, not rotatable — and, far more practically, Box counts its API rate limits per
/// application, so one compiled-in registration would have every installation in the world
/// sharing one allowance. Registered per installation, each one has its own (RD-106-04, rule 8).
///
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be. Box requires an exact
/// match, port included.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("box_oauth.{code}")),
        params: Vec::new(),
    }
}

/// A value nobody can recompute: the host's random bytes, base64url-encoded.
///
/// An empty answer means the host refused, and a sign-in is failed rather than continued with a
/// value the plugin made up.
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

/// The marker that stands for the client secret of the person's own application.
fn client_secret_template() -> String {
    format!("{{{{secret:{CLIENT_SECRET_REFERENCE}}}}}")
}

/// Refuses before any request when the account carries no registered application.
///
/// `secret-available` reports whether a credential exists and never what it is, which is
/// exactly the question worth asking here. Asking it first is what turns a puzzle into an
/// instruction: without it Box would answer `invalid_client`, which says the application was
/// refused rather than that there is none, and the person would go looking for a fault in a
/// registration they never made.
fn require_registered_application(account_id: &str) -> Result<(), Failure> {
    if host::secret_available(account_id, CLIENT_SECRET_REFERENCE) {
        return Ok(());
    }
    Err(refuse(
        "client_not_configured",
        "this account has no registered Box application to sign in with".to_owned(),
        FailureKind::AuthRequired,
    ))
}

fn form(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .filter(|(_, value)| !value.is_empty())
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

/// The `Retry-After` Box sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// Turns a token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "Box could not be reached". That never arrives as an
/// answer — `http-request` fails instead, the guest returns that failure with `?`, and the host
/// keeps the stored token and tries again later. Only a refusal Box actually made becomes
/// `failed`, because a plugin that confused the two would sign people out of their Box every
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
            // credential is already kept, so saying it first would be a lie. The `box` provider
            // declares a slot of its own for what a flow fills, so this lands beside the client
            // secret rather than over it. From this call on, both values are redacted out of
            // anything this plugin logs.
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
                format!("box refused the sign-in: {}", flow::sanitize_error(&error)),
                FailureKind::AuthRequired,
            )))
        }
        // A rate limit says nothing about the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds.unwrap_or(30))),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("box answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

/// The one refusal both device functions answer with.
fn no_device_flow() -> Failure {
    refuse(
        "flow_unsupported",
        "box is signed in through the browser, not with a device code".to_owned(),
        FailureKind::Unsupported,
    )
}

impl Guest for Component {
    /// Builds the address to send the person to.
    ///
    /// No `scope` parameter, and that is a decision rather than an omission. Box takes the
    /// scopes from the application's own configuration when none is named, and narrowing them
    /// from here would either repeat what the person already ticked in the Developer Console or
    /// ask for something their application does not have — which Box refuses outright. Least
    /// privilege lives where Box put it: the account's catalogue text says to tick the read
    /// scope and nothing that writes.
    fn begin(
        account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<AuthorizationRequest, Failure> {
        require_registered_application(&account_id)?;
        // From the host's random source and from nothing else. A value computed from the
        // account and the time of day is a value anybody holding them can recompute.
        let state = unguessable_value()?;
        let authorization_url = format!(
            "{AUTHORIZE_ENDPOINT}?response_type=code&client_id={}&redirect_uri={}&state={}",
            // Left as the marker: the host substitutes this installation's own client id here,
            // because a guest has no way to read a configured value.
            CLIENT_ID_MARKER,
            pkce::percent_encode(REDIRECT_URI),
            pkce::percent_encode(&state),
        );
        Ok(AuthorizationRequest {
            authorization_url,
            // The host stores this and compares it with what the callback quotes. A callback
            // naming a value no flow claims matches nothing and is dropped.
            state,
            // Box's authorization code is good for thirty seconds; the flow a person has to
            // walk through is not. Ten minutes is how long the flow may sit open, which is what
            // this bounds.
            expires_in_seconds: Some(600),
            // Nothing has to survive to `poll`: Box takes no verifier, and the client secret it
            // does take is the account's, which the host substitutes.
            flow_state: None,
        })
    }

    /// Exchanges the code the callback carried.
    fn poll(
        account_id: String,
        code: String,
        _flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        require_registered_application(&account_id)?;
        let secret = client_secret_template();
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", CLIENT_ID_MARKER),
                ("client_secret", &secret),
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
    /// Neither credential is in this function: `credential_ref` names the refresh token and
    /// [`CLIENT_SECRET_REFERENCE`] the application's secret, and the host substitutes both
    /// values into `{{secret:<reference>}}` on the way out. This is what a Box account lives
    /// on — its access tokens last about an hour — so without it every download started later
    /// than that after a sign-in would ask somebody to sign in again.
    fn refresh(
        account_id: String,
        credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        require_registered_application(&account_id)?;
        let Some(reference) = credential_ref.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "refresh_refused",
                "this account has no stored sign-in to renew".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        let secret = client_secret_template();
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &format!("{{{{secret:{reference}}}}}")),
                ("client_id", CLIENT_ID_MARKER),
                ("client_secret", &secret),
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
