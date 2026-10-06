//! The component: AllDebrid's PIN flow, one call per step.
//!
//! The same contract as a device flow in a different shape — a short PIN and an address, and a
//! separate opaque value the application polls with. The opaque value travels back through
//! `flow-state`, because a guest is instantiated fresh for every call and remembers nothing.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_auth::{
    AuthState, Guest, UserPrompt, credentials,
    http::{self, RequestQuery},
    refuse,
    types::{Failure, FailureKind},
};

use crate::flow;

struct Component;

/// The prefix of every translation code this plugin reports.
const SLUG: &str = "alldebrid_auth";

const PIN_GET: &str = "https://api.alldebrid.com/v4/pin/get";
const PIN_CHECK: &str = "https://api.alldebrid.com/v4/pin/check";

fn query(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect()
}

impl Guest for Component {
    fn begin(_account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        let response =
            http::http_request("GET", PIN_GET, &query(&[("agent", flow::AGENT)]), &[], &[])?;
        let body = String::from_utf8_lossy(&response.body);
        if let Some(message) = flow::error(&body) {
            return Err(refuse(
                SLUG,
                flow::refusal_code(&message),
                format!(
                    "the provider refused the sign-in: {}",
                    flow::sanitize_error(&message)
                ),
                FailureKind::Permanent,
            ));
        }
        let Some(pin) = flow::pin(&body) else {
            return Err(refuse(
                SLUG,
                "bad_reply",
                format!(
                    "the provider answered {} to the sign-in request",
                    response.status
                ),
                FailureKind::Permanent,
            ));
        };
        Ok(AuthState::UserAction(UserPrompt {
            verification_url: pin.user_url,
            user_code: Some(pin.pin),
            expires_in_seconds: pin.expires_in,
            // The check value, not the PIN: this is what the poll is made with, and unlike the
            // PIN it is never shown to anybody.
            flow_state: Some(pin.check),
        }))
    }

    fn poll(account_id: String, flow_state: Option<String>) -> Result<AuthState, Failure> {
        let Some(check) = flow_state.filter(|check| !check.is_empty()) else {
            return Ok(AuthState::Failed(refuse(
                SLUG,
                "flow_expired",
                "the sign-in has nothing to continue with",
                FailureKind::AuthRequired,
            )));
        };
        let response = http::http_request(
            "GET",
            PIN_CHECK,
            &query(&[("agent", flow::AGENT), ("check", &check)]),
            &[],
            &[],
        )?;
        let body = String::from_utf8_lossy(&response.body);
        match flow::poll(&body) {
            flow::PollOutcome::Authorized(key) => {
                // Stored before `Authorized` is returned: the host takes that answer to mean
                // the credential is already kept.
                credentials::store_token(&account_id, &key)?;
                Ok(AuthState::Authorized)
            }
            // AllDebrid names no interval, so this is the plugin's own: often enough to feel
            // immediate, seldom enough not to hammer a service for minutes on end.
            flow::PollOutcome::Pending => Ok(AuthState::Pending(4)),
            // The provider's own code survives; its prose does not. `sanitize_error`
            // drops anything that is not code-shaped whole rather than filtering it, because
            // filtering an answer that echoed a credential would keep its digits.
            flow::PollOutcome::Failed(reason) => Ok(AuthState::Failed(refuse(
                SLUG,
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

plugin_guest_auth::auth_plugin!(Component);
