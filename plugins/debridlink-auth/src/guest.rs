//! The component: Debrid-Link's OAuth device flow, one call per step.
//!
//! A guest is instantiated fresh for every invocation and remembers nothing of its own, so the
//! device code `begin` obtained travels back through `flow-state`: the host stores it verbatim
//! and hands it to `poll`. That is what makes a sign-in survive a service restart.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "auth-plugin",
});

use exports::rdownloader::plugin::auth::{AuthState, Guest, UserPrompt};
use rdownloader::plugin::{
    credentials,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::flow;

struct Component;

const DEVICE_ENDPOINT: &str = "https://debrid-link.com/api/oauth/device/code";
const TOKEN_ENDPOINT: &str = "https://debrid-link.com/api/oauth/token";

/// The scopes rDownloader needs, and no more: resolving links and reading the account.
const SCOPE: &str = "get.post.downloader get.account";

fn form(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect()
}

/// A failure carrying a stable translation code and nothing a provider wrote.
///
/// The code is a parameter now (RD-106-01). It used to be `debridlink_auth.flow_expired` for
/// every refusal there is, so an unreadable answer and a blocked account both told the
/// person their code had expired — and the one thing they could act on, starting again,
/// was the one thing that could not help.
fn refuse(code: &str, message: impl Into<String>, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.into(),
        code: Some(format!("debridlink_auth.{code}")),
        params: Vec::new(),
    }
}

impl Guest for Component {
    fn begin(_account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        let response = http::http_request(
            "POST",
            DEVICE_ENDPOINT,
            &form(&[("client_id", flow::CLIENT_ID), ("scope", SCOPE)]),
            &[],
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        let Some(code) = flow::device_code(&body) else {
            return Err(refuse(
                "bad_reply",
                format!(
                    "the provider answered {} to the sign-in request",
                    response.status
                ),
                FailureKind::Permanent,
            ));
        };
        // The address goes back as the provider gave it. The host refuses one outside the
        // domains this manifest declares, which is what stops a plugin sending somebody to a
        // sign-in page of its own choosing.
        Ok(AuthState::UserAction(UserPrompt {
            verification_url: code.verification_url,
            user_code: Some(code.user_code),
            expires_in_seconds: code.expires_in,
            // The device code, not the user code: this is what the next poll is made with,
            // and it is never shown to anybody.
            flow_state: Some(code.device_code),
        }))
    }

    fn poll(account_id: String, flow_state: Option<String>) -> Result<AuthState, Failure> {
        // Without the device code there is nothing to poll with. Failing says so once instead
        // of asking the provider a question it cannot answer, for ever.
        let Some(device_code) = flow_state.filter(|code| !code.is_empty()) else {
            return Ok(AuthState::Failed(refuse(
                "flow_expired",
                "the sign-in has no code to continue with",
                FailureKind::AuthRequired,
            )));
        };
        let response = http::http_request(
            "POST",
            TOKEN_ENDPOINT,
            &form(&[
                ("client_id", flow::CLIENT_ID),
                ("code", &device_code),
                ("grant_type", "http://oauth.net/grant_type/device/1.0"),
            ]),
            &[RequestHeader {
                name: "Accept".to_owned(),
                value_template: "application/json".to_owned(),
            }],
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        match flow::poll(&body) {
            flow::PollOutcome::Authorized(token) => {
                // Stored before `Authorized` is returned: the host takes that answer to mean
                // the credential is already kept, so saying it first would be a lie.
                credentials::store_token(&account_id, &token)?;
                Ok(AuthState::Authorized)
            }
            flow::PollOutcome::Pending(interval) => Ok(AuthState::Pending(interval.unwrap_or(5))),
            // The provider's own code survives; its prose does not. `sanitize_error`
            // drops anything that is not code-shaped whole rather than filtering it, because
            // filtering an answer that echoed a credential would keep its digits.
            flow::PollOutcome::Failed(reason) => Ok(AuthState::Failed(refuse(
                flow::refusal_code(&reason),
                format!(
                    "the provider refused the sign-in: {}",
                    flow::sanitize_error(&reason)
                ),
                FailureKind::AuthRequired,
            ))),
        }
    }
}

export!(Component);
