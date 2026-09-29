//! The component: a PIN asked for, shown, and polled until it turns into an API key.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "auth-plugin",
});

use exports::rdownloader::plugin::auth::{AuthState, Guest, UserPrompt};
use rdownloader::plugin::{
    credentials,
    http::{self, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::Check;

struct Component;

/// Seconds between polls. The host does the waiting, so nothing here sleeps.
const POLL_SECONDS: u64 = 5;

impl Guest for Component {
    /// Starts the flow and hands back what the person has to do.
    ///
    /// The verification address is shown as the provider gave it, never opened automatically,
    /// and it must be on a domain your manifest declares — the host refuses anything else
    /// rather than sending someone to an address a plugin invented.
    fn begin(_account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        // A failure to reach the provider comes back through `?` as the host's own transient
        // failure: nothing was refused, so the person can simply try again.
        let response = http::http_request("GET", crate::PIN_ENDPOINT, &[], &[], &[])?;
        let Some(pin) = crate::read_pin(&String::from_utf8_lossy(&response.body)) else {
            return Ok(AuthState::Failed(refuse(
                FailureKind::Permanent,
                "bad_reply",
                "the provider did not hand out a PIN",
            )));
        };
        Ok(AuthState::UserAction(UserPrompt {
            verification_url: pin.user_url.clone(),
            user_code: Some(pin.pin.clone()),
            expires_in_seconds: pin.expires_in,
            // What the next poll is made with. Stored by the host and never shown; it is not a
            // credential, and a token must never travel here.
            flow_state: Some(pin.flow_state()),
        }))
    }

    /// Asks the provider whether the person has confirmed yet.
    ///
    /// Once the provider hands over a key, store it before returning `Authorized` — the host
    /// takes that answer to mean the credential is already kept.
    fn poll(account_id: String, flow_state: Option<String>) -> Result<AuthState, Failure> {
        // Without what `begin` handed over there is nothing to poll with. Failing says so once
        // instead of asking the provider a question it cannot answer, for ever.
        let Some((pin, check)) = flow_state.as_deref().and_then(crate::from_flow_state) else {
            return Ok(AuthState::Failed(refuse(
                FailureKind::AuthRequired,
                "flow_expired",
                "the sign-in has nothing to continue with",
            )));
        };
        let query = [("pin", pin), ("check", check)].map(|(name, value)| RequestQuery {
            name: name.to_owned(),
            value_template: value.to_owned(),
        });
        let response = http::http_request("GET", crate::CHECK_ENDPOINT, &query, &[], &[])?;
        match crate::read_check(&String::from_utf8_lossy(&response.body)) {
            Check::Activated(key) => {
                credentials::store_token(&account_id, &key)?;
                Ok(AuthState::Authorized)
            }
            Check::Waiting => Ok(AuthState::Pending(POLL_SECONDS)),
            Check::Expired => Ok(AuthState::Failed(refuse(
                FailureKind::AuthRequired,
                "flow_expired",
                "the PIN expired before it was confirmed",
            ))),
            Check::Unreadable => Ok(AuthState::Failed(refuse(
                FailureKind::Permanent,
                "bad_reply",
                "the provider sent a reply this plugin could not read",
            ))),
        }
    }
}

/// A failure carrying a stable translation code and nothing the provider wrote.
fn refuse(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

export!(Component);
