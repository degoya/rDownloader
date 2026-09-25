//! The component: Real-Debrid's OAuth2 device flow, and the renewal that outlives it.
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

use crate::flow;

/// Where a device sign-in asks for the code the person types.
const DEVICE_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/device/code";
/// Where the device code and the refresh material are exchanged for tokens.
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";
/// Real-Debrid's device grant, spelled as its documentation spells it. It is not the RFC 8628
/// URN, and sending that instead is refused.
const GRANT_TYPE: &str = "http://oauth.net/grant_type/device/1.0";

/// The application is the person's own, and nothing about it is compiled in.
///
/// A client id travels as `{{username}}` — it identifies an application rather than a person,
/// which is what the account's username field is for — and the client secret as the marker
/// below. Both are substituted by the host on the way out and reach this plugin never.
///
/// **Why no pair is shipped.** An OAuth client secret in an open-source repository is not a
/// secret: it would stand in the git history, in every signed `.rdplug`, and in every release
/// artefact anybody downloads. Worse than the disclosure is the sharing — Real-Debrid's rate
/// limits are per application, so one shipped registration would put every installation in the
/// world into one bucket and let any of them exhaust it for all the others. Registered per
/// installation means each has its own limits, and its own revocation.
const CLIENT_ID_TEMPLATE: &str = "{{username}}";

/// The account's own credential: the client secret of the application the person registered.
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

/// The marker that stands for the client secret of the person's own application.
fn client_secret_template() -> String {
    format!("{{{{secret:{CLIENT_SECRET_REFERENCE}}}}}")
}

/// Refuses before any request when the account carries no registered application.
///
/// `secret-available` reports whether a credential exists and never what it is, which is
/// exactly the question worth asking here. Asking it first is what turns a puzzle into an
/// instruction: without it the provider would answer `invalid_client`, which says the
/// application was refused rather than that there is none, and the person would go looking for
/// a fault in a registration they never made.
fn require_registered_application(account_id: &str) -> Result<(), Failure> {
    if host::secret_available(account_id, CLIENT_SECRET_REFERENCE) {
        return Ok(());
    }
    Err(refuse(
        "client_not_configured",
        "this account has no registered Real-Debrid application to sign in with".to_owned(),
        FailureKind::AuthRequired,
    ))
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

/// One exchange at the token endpoint: the device grant, with whatever `code` stands for this
/// time — the device code on a sign-in, the refresh material on a renewal.
///
/// Both calls are the same request with a different `code`, which is Real-Debrid's own design
/// and not a shortcut taken here: the renewal grant type is the device grant type.
fn exchange(account_id: &str, code: &str) -> Result<TokenOutcome, Failure> {
    let secret = client_secret_template();
    let response = http::http_request(
        "POST",
        TOKEN_ENDPOINT,
        &query(&[
            ("client_id", CLIENT_ID_TEMPLATE),
            ("client_secret", &secret),
            ("code", code),
            ("grant_type", GRANT_TYPE),
        ]),
        &accept_json(),
        &[],
    )?;
    outcome(
        account_id,
        response.status,
        retry_after(&response.headers).as_deref(),
        &String::from_utf8_lossy(&response.body),
    )
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
        flow::TokenAnswer::Refused { error, api_code } => {
            let code = flow::refusal_code(&error, api_code);
            Ok(TokenOutcome::Failed(refuse(
                code,
                format!(
                    "the provider refused the sign-in: {}",
                    flow::sanitize_error(&error)
                ),
                FailureKind::AuthRequired,
            )))
        }
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
    /// `credential_ref` is unused because this plugin already knows what to name: the client
    /// secret sits in the account's own declared slot, so the marker is the same every time and
    /// the host resolves it per account. What ties the flow to an account is the device code
    /// the host stores next to it.
    fn device_begin(
        account_id: String,
        _credential_ref: Option<String>,
    ) -> Result<DeviceAuthorization, Failure> {
        require_registered_application(&account_id)?;
        let response = http::http_request(
            "GET",
            DEVICE_ENDPOINT,
            &query(&[("client_id", CLIENT_ID_TEMPLATE)]),
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
            // The device code, never the user code: this is what the poll is made with, and it
            // is never shown to anybody.
            flow_state: Some(code.device_code),
        })
    }

    /// Asks whether the person has confirmed yet, with the device code handed back.
    fn device_poll(
        account_id: String,
        flow_state: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        // Without the device code there is nothing to poll with. Failing says so once instead
        // of asking the provider a question it cannot answer, for ever.
        require_registered_application(&account_id)?;
        let Some(device_code) = flow_state.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "code_expired",
                "the sign-in has no device code to continue with".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        exchange(&account_id, &device_code)
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// The refresh token is never in this function: `credential_ref` names it, and the host
    /// substitutes the value into `{{secret:<reference>}}` on the way out. Real-Debrid takes it
    /// in the same `code` field the device code went into, under the same grant type — which is
    /// why one `exchange` serves both.
    fn refresh(
        account_id: String,
        credential_ref: Option<String>,
    ) -> Result<TokenOutcome, Failure> {
        require_registered_application(&account_id)?;
        let Some(reference) = credential_ref.filter(|value| !value.is_empty()) else {
            return Ok(TokenOutcome::Failed(refuse(
                "sign_in_refused",
                "this account has no stored sign-in to renew".to_owned(),
                FailureKind::AuthRequired,
            )));
        };
        exchange(&account_id, &format!("{{{{secret:{reference}}}}}"))
    }
}

export!(Component);
