//! The component: Real-Debrid's open-source device flow, and the renewal that outlives it.
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

use crate::{
    flow,
    form::{self, Field},
};

/// Where a device sign-in asks for the code the person types.
const DEVICE_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/code";
/// Where a confirmed device code is exchanged for the person's own client credentials.
const CREDENTIALS_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/credentials";
/// Where the device code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";
/// Real-Debrid's device grant, spelled as its documentation spells it. It is not the RFC 8628
/// URN, and sending that instead is refused.
const GRANT_TYPE: &str = "http://oauth.net/grant_type/device/1.0";

/// Real-Debrid's public client id for open-source applications (RD-150-09).
///
/// Not a secret, and published as such: it only opens the device flow, and what the person
/// confirms there is a client id and client secret of their *own*, issued because the request
/// says `new_credentials=yes`. Rate limits and revocation hang off those personal credentials,
/// so no installation shares a bucket with another -- the objection RD-106-03 had against a
/// shipped registration does not apply. No client secret is ever shipped.
const PUBLIC_CLIENT_ID: &str = "X245A4XAIBGVM";

/// The personal client id the flow issued, kept by the host as a named part of the sign-in.
const CLIENT_ID_REFERENCE: &str = "realdebrid_client_id";

/// The personal client secret the flow issued, kept the same way.
const CLIENT_SECRET_REFERENCE: &str = "realdebrid_client_secret";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("realdebrid_auth.{code}")),
        params: Vec::new(),
    }
}

fn query(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect()
}

/// The marker the host expands into the value stored under `reference`.
fn secret_template(reference: &str) -> String {
    format!("{{{{secret:{reference}}}}}")
}

fn accept_json() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "application/json".to_owned(),
    }]
}

/// What the token request sends besides its body: the body's type, which is also what makes the
/// host look for the markers in it and encode what it fills in for a form.
fn form_headers() -> Vec<RequestHeader> {
    let mut headers = accept_json();
    headers.push(RequestHeader {
        name: "Content-Type".to_owned(),
        value_template: "application/x-www-form-urlencoded".to_owned(),
    });
    headers
}

/// The `Retry-After` a provider sent, if it sent one.
fn retry_after(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.clone())
}

/// One exchange at the token endpoint: the device grant, with whatever `code` stands for this
/// time — the device code on a sign-in, the refresh material on a renewal.
///
/// Both calls are the same request with a different `code`, which is Real-Debrid's own design
/// and not a shortcut taken here: the renewal grant type is the device grant type. The client is
/// the person's own on both, named and never held: the host expands the two parts the sign-in
/// stored, towards `api.real-debrid.com` and nowhere else.
///
/// All four fields go in the form body. Real-Debrid reads them from there alone and answers the
/// same fields in the query string with `parameter_missing` -- after the person has confirmed
/// the device, which is what 1.5.1 did to every sign-in.
fn exchange(account_id: &str, code: Field<'_>) -> Result<TokenOutcome, Failure> {
    let client_id = secret_template(CLIENT_ID_REFERENCE);
    let client_secret = secret_template(CLIENT_SECRET_REFERENCE);
    let body = form::body(&[
        ("client_id", Field::Marker(&client_id)),
        ("client_secret", Field::Marker(&client_secret)),
        ("code", code),
        ("grant_type", Field::Value(GRANT_TYPE)),
    ]);
    let response = http::http_request(
        "POST",
        TOKEN_ENDPOINT,
        &[],
        &form_headers(),
        body.as_bytes(),
    )?;
    outcome(
        account_id,
        response.status,
        retry_after(&response.headers).as_deref(),
        &String::from_utf8_lossy(&response.body),
    )
}

/// A refusal the provider made, as the outcome that ends the flow.
fn refused(error: &str, api_code: Option<u64>) -> TokenOutcome {
    TokenOutcome::Failed(refuse(
        flow::refusal_code(error, api_code),
        format!(
            "the provider refused the sign-in: {}",
            flow::sanitize_error(error)
        ),
        FailureKind::AuthRequired,
    ))
}

/// Asks whether the device code was confirmed, and keeps the personal client it was issued.
///
/// The two values are handed to the host the moment they arrive and are named, never held,
/// from then on: `store-flow-secret` keeps each as a part of this account's sign-in, and the
/// token exchange right after names them as markers like every later renewal will. Stored
/// before the exchange, so a token endpoint that is unreachable for a moment costs a poll and
/// not the client -- the next poll asks for the credentials again and gets the same pair.
fn claim_credentials(account_id: &str, device_code: &str) -> Result<TokenOutcome, Failure> {
    let response = http::http_request(
        "GET",
        CREDENTIALS_ENDPOINT,
        &query(&[("client_id", PUBLIC_CLIENT_ID), ("code", device_code)]),
        &accept_json(),
        &[],
    )?;
    match flow::read_credentials_answer(
        response.status,
        retry_after(&response.headers).as_deref(),
        &String::from_utf8_lossy(&response.body),
    ) {
        flow::CredentialsAnswer::Issued {
            client_id,
            client_secret,
        } => {
            credentials::store_flow_secret(account_id, CLIENT_ID_REFERENCE, &client_id)?;
            credentials::store_flow_secret(account_id, CLIENT_SECRET_REFERENCE, &client_secret)?;
            exchange(account_id, Field::Value(device_code))
        }
        flow::CredentialsAnswer::Refused { error, api_code } => Ok(refused(&error, api_code)),
        // Nobody has confirmed yet, or the request budget is spent: a wait either way.
        flow::CredentialsAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds)),
    }
}

/// Turns a token endpoint's answer into the outcome the host acts on.
///
/// Note what is *not* here: a case for "the provider could not be reached". That never arrives
/// as an answer — `http-request` fails instead, the guest returns that failure with `?`, and the
/// host keeps the stored token and tries again later. Only a refusal the provider actually made
/// becomes `failed`.
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
        flow::TokenAnswer::Refused { error, api_code } => Ok(refused(&error, api_code)),
        // Nobody has confirmed yet, or the request budget is spent. Neither says anything
        // about the credential, so both are a wait and not a failure.
        flow::TokenAnswer::Busy(seconds) => Ok(TokenOutcome::Pending(seconds)),
        flow::TokenAnswer::Unreadable(status) => Ok(TokenOutcome::Failed(refuse(
            "bad_reply",
            format!("the provider answered {status} to the token request"),
            FailureKind::Permanent,
        ))),
    }
}

/// The refusal both redirect functions answer with.
///
/// The manifest offers `device` and nothing else, so the host never calls these: `require_flow`
/// refuses first, with a typed error the caller can act on. The world still demands the
/// exports, and a stable code is a better thing to find here than a trap.
fn no_redirect_entrance() -> Failure {
    refuse(
        "redirect_unsupported",
        "this provider signs in with a device code and has no redirect".to_owned(),
        FailureKind::Unsupported,
    )
}

impl Guest for Component {
    fn begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<AuthorizationRequest, Failure> {
        Err(no_redirect_entrance())
    }

    fn poll(
        _account_id: String,
        _code: String,
        _flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        Err(no_redirect_entrance())
    }

    /// Asks the provider for a device code and reports what the person has to be shown.
    ///
    /// There is no PKCE here, and that is not an omission. PKCE binds an authorization code to
    /// the client that asked for it, and a device flow has no authorization code and no
    /// redirect to intercept — what binds this exchange is the device code itself, which the
    /// provider issued to this client and nobody else ever sees.
    ///
    /// Nothing has to exist on the account first: the public client id opens the flow and
    /// `new_credentials=yes` asks Real-Debrid to issue the person a client of their own once
    /// they confirm. `credential_ref` is unused; what ties the flow to an account is the device
    /// code the host stores next to it.
    fn device_begin(
        _account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<DeviceAuthorization, Failure> {
        let response = http::http_request(
            "GET",
            DEVICE_ENDPOINT,
            &query(&[("client_id", PUBLIC_CLIENT_ID), ("new_credentials", "yes")]),
            &accept_json(),
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        // Said as what it is, with the provider's own wait: a spent budget is not an answer
        // this plugin failed to read, and the person can simply try again a little later.
        if flow::is_rate_limited(response.status, &body) {
            return Err(refuse(
                "rate_limited",
                "the provider asked for fewer requests before a sign-in can start".to_owned(),
                FailureKind::RateLimited(
                    retry_after(&response.headers).and_then(|value| value.trim().parse().ok()),
                ),
            ));
        }
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
            // The device code, never the user code: this is what the poll is made with, and it
            // is never shown to anybody.
            flow_state: Some(code.device_code),
        })
    }

    /// Asks whether the person has confirmed yet, with the device code handed back; once they
    /// have, keeps the personal client they were issued and exchanges the code for a token.
    fn device_poll(
        account_id: String,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        // Without the device code there is nothing to poll with. Failing says so once instead
        // of asking the provider a question it cannot answer, for ever.
        let Some(device_code) = flow_state.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "code_expired",
                "the sign-in has no device code to continue with".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        claim_credentials(&account_id, &device_code)
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// The refresh token is never in this function: `credential_ref` names it, and the host
    /// substitutes the value into `{{secret:<reference>}}` on the way out. Real-Debrid takes it
    /// in the same `code` field the device code went into, under the same grant type — which is
    /// why one `exchange` serves both.
    ///
    /// A renewal also needs the personal client the sign-in kept. `secret-available` says
    /// whether it is there without saying what it is, and an account without it refuses here
    /// rather than sending the provider a request it can only reject.
    fn refresh(
        account_id: String,
        credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        let has_client = [CLIENT_ID_REFERENCE, CLIENT_SECRET_REFERENCE]
            .into_iter()
            .all(|reference| host::secret_available(&account_id, reference));
        if !has_client {
            return Ok(TokenOutcome::Failed(refuse(
                "sign_in_refused",
                "this account keeps no client of its own to renew with".to_owned(),
                FailureKind::AuthRequired,
            )));
        }
        let Some(reference) = credential_ref.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "sign_in_refused",
                "this account has no stored sign-in to renew".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        exchange(&account_id, Field::Marker(&secret_template(&reference)))
    }
}

export!(Component);
