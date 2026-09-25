//! The component: pCloud's authorization code, redeemed at whichever installation issued it.
//!
//! Redirect only. pCloud has no device flow, so `device-begin` and `device-poll` answer with a
//! stable code rather than a trap — the world requires them to exist, and the manifest's
//! `oauth_flows = ["redirect"]` means the host never calls them anyway.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "oauth-plugin",
});

use exports::rdownloader::plugin::oauth::{
    AuthorizationRequest, DeviceAuthorization, Guest, TokenOutcome,
};
use pcloud_common::address::Region;
use rdownloader::plugin::{
    credentials, host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{flow, pkce};

/// What the host replaces with this installation's own pCloud application id. Spelled here
/// rather than imported because a guest links nothing of the host's.
const CLIENT_ID_MARKER: &str = "{{client_id}}";

/// The account's own credential: the secret of the application the person registered.
const CLIENT_SECRET_REFERENCE: &str = "pcloud_client_secret";

/// Where the person agrees. On a domain `manifest.toml` declares, which is what stops a signed
/// plugin sending somebody to a sign-in page of its own choosing.
///
/// One page for both data centres: pCloud decides which the account belongs to and states it
/// in the redirect as `hostname` and `locationid`.
const AUTHORIZE_ENDPOINT: &str = "https://my.pcloud.com/oauth2/authorize";

/// The application **this installation** registered for itself in pCloud's console, written as
/// the markers the host expands from the account (RD-106-04, rule 8).
///
/// There is deliberately no application id and no secret in this file, and it is worth saying
/// why rather than leaving it to be rediscovered. A pair compiled in here would sit in the git
/// history of a public repository, in every signed `.rdplug` and in every release artefact —
/// and, far more practically, pCloud rate-limits per application, so one compiled-in
/// registration would have every installation in the world sharing one allowance. Registered
/// per installation, each one has its own, and its own revocation.
///
/// An application id identifies the application to pCloud, not the person to the application:
/// pCloud publishes it in the address the person is sent to. So it is not a credential, it is
/// stored in the clear as the account's username, and the host puts it into the authorization
/// URL as well as into the exchange. The secret beside it *is* a credential and lives in the
/// vault, because pCloud's token endpoint is a confidential client and offers no PKCE
/// alternative.
///
/// One fixed address for the redirect, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be.
const REDIRECT_URI: &str = "http://127.0.0.1:8710/api/v1/oauth/callback";

/// How long the address the person is sent to is worth following.
const AUTHORIZATION_LIFETIME: u64 = 600;

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("pcloud_oauth.{code}")),
        params: Vec::new(),
    }
}

/// A value nobody can recompute: the host's random bytes, base64url-encoded.
///
/// An empty answer means the host refused, and a sign-in is failed rather than continued with
/// a value the plugin made up. Neither the account nor the time of day takes part: a value
/// computed from those is a value anybody holding them can recompute.
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

/// Refuses before any request when the account carries no registered application.
///
/// `secret-available` reports whether a credential exists and never what it is, which is
/// exactly the question worth asking here. Asking it first is what turns a puzzle into an
/// instruction: without it pCloud would answer with a number that says the application was
/// refused rather than that there is none, and the person would go looking for a fault in a
/// registration they never made.
fn require_registered_application(account_id: &str) -> Result<(), Failure> {
    if host::secret_available(account_id, CLIENT_SECRET_REFERENCE) {
        return Ok(());
    }
    Err(refuse(
        "client_not_configured",
        "this account has no registered pCloud application to sign in with".to_owned(),
        FailureKind::AuthRequired,
    ))
}

fn query(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
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

/// The `Retry-After` pCloud sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// One redemption attempt, at one of pCloud's two installations.
fn redeem(region: Region, code: &str) -> Result<flow::TokenAnswer, Failure> {
    let secret = format!("{{{{secret:{CLIENT_SECRET_REFERENCE}}}}}");
    let response = http::http_request(
        "POST",
        &format!("{}/oauth2_token", region.api()),
        &query(&[
            ("client_id", CLIENT_ID_MARKER),
            ("client_secret", &secret),
            ("code", code),
        ]),
        &accept_json(),
        &[],
    )?;
    Ok(flow::read_token_answer(
        response.status,
        retry_after(&response.headers).as_deref(),
        &String::from_utf8_lossy(&response.body),
    ))
}

/// Turns a token answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "pCloud could not be reached". That never arrives as an
/// answer — `http-request` fails instead, the guest returns that failure with `?`, and the host
/// keeps the stored token and tries again later. Only a refusal pCloud actually made becomes
/// `failed`, because a plugin that confused the two would sign people out of their pCloud every
/// time their connection dropped.
fn outcome(account_id: &str, answer: flow::TokenAnswer) -> Result<TokenOutcome, Failure> {
    match answer {
        flow::TokenAnswer::Granted { access_token } => {
            // Stored before `Authorized` is returned: the host takes that answer to mean the
            // credential is already kept, so saying it first would be a lie. From this call on
            // the value is redacted out of anything this plugin logs.
            //
            // No renewal material and no expiry, and neither is an omission: pCloud issues no
            // refresh token, and the access token it issues lives until it is revoked. Saying
            // so is also what keeps the host's renewal sweep away from an account it could
            // never renew — the sweep selects flows that carry both.
            credentials::store_oauth_token(account_id, &access_token, None, None)?;
            Ok(TokenOutcome::Authorized)
        }
        flow::TokenAnswer::Refused(result) => Ok(TokenOutcome::Failed(refuse(
            flow::refusal_code(result),
            format!("pcloud refused the sign-in with result {result}"),
            FailureKind::AuthRequired,
        ))),
        // A rate limit says nothing about the credential, so it is a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds.unwrap_or(30))),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("pcloud answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

/// The one refusal both device functions answer with.
fn no_device_flow() -> Failure {
    refuse(
        "flow_unsupported",
        "pcloud is signed in through the browser, not with a device code".to_owned(),
        FailureKind::Unsupported,
    )
}

impl Guest for Component {
    /// Builds the address to send the person to.
    ///
    /// No `code_challenge`: pCloud's token endpoint documents `client_id`, `client_secret` and
    /// `code`, and offers PKCE in neither half of the flow. What protects the exchange instead
    /// is the application secret, which is why this plugin refuses to start without one.
    fn begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<AuthorizationRequest, Failure> {
        let state = unguessable_value()?;
        let authorization_url = format!(
            "{AUTHORIZE_ENDPOINT}?response_type=code&client_id={}&redirect_uri={}&state={}",
            // Left as the marker: the host substitutes this installation's own application id
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
            expires_in_seconds: Some(AUTHORIZATION_LIFETIME),
            // Nothing to carry: pCloud offers no PKCE, so there is no verifier, and inventing
            // a value to put here would be bookkeeping that proves nothing.
            flow_state: None,
        })
    }

    /// Exchanges the code the callback carried, at whichever installation issued it.
    ///
    /// pCloud states the account's data centre in the redirect, but the contract hands this
    /// function the code alone, so the installation is found rather than told: the code is
    /// offered to `api.pcloud.com` first and to `eapi.pcloud.com` if that one does not know
    /// it. A code an installation never issued cannot be spent there, so the first attempt
    /// consumes nothing.
    fn poll(
        account_id: String,
        code: String,
        _flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        require_registered_application(&account_id)?;
        let mut refusal = None;
        for region in Region::Us.both_from() {
            let answer = redeem(region, &code)?;
            match answer {
                flow::TokenAnswer::Refused(result)
                    if refusal.is_none()
                        && pcloud_common::api::Category::of(result).may_be_the_other_region() =>
                {
                    host::log(
                        "debug",
                        "pcloud did not issue this code at the first data centre; \
                         asking the other one",
                    );
                    refusal = Some(flow::TokenAnswer::Refused(result));
                }
                answer => return outcome(&account_id, answer),
            }
        }
        match refusal {
            Some(answer) => outcome(&account_id, answer),
            None => Ok(TokenOutcome::Failed(refuse(
                "bad_reply",
                "pcloud answered neither data centre's exchange".to_owned(),
                FailureKind::Permanent,
            ))),
        }
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

    /// There is nothing to renew from, and saying so is the honest answer.
    ///
    /// pCloud issues no refresh token: its access token lives until somebody revokes it, and
    /// when that happens the only way back is a new sign-in. The host does not ask for this
    /// either — its renewal sweep selects flows carrying both refresh material and an expiry,
    /// and `poll` deliberately stores neither — so this is the answer to a question that should
    /// not arrive, rather than a gap in the flow.
    fn refresh(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        Ok(TokenOutcome::Failed(refuse(
            "renewal_unsupported",
            "pcloud issues no renewal material; this account has to be signed in again".to_owned(),
            FailureKind::AuthRequired,
        )))
    }
}

export!(Component);
