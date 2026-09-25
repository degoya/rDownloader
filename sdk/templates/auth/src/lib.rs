//! A scaffold authentication provider. It compiles, packages and passes conformance as it is.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - You never see a credential. `credential-ref` is an opaque handle, and the token you
//!   obtain goes back through `credentials.store-token`, which writes it where the account's
//!   provider keeps it. There is no call that reads one back.
//! - The account you are handed is the only one you can write to. Naming another is refused,
//!   so a flow cannot reach past the account it was started for.
//! - You remember nothing between calls: a guest is instantiated fresh for every one. Whatever
//!   `poll` needs from `begin` — a device code, a PIN check value — goes back in
//!   `flow-state`, and the host hands it over verbatim on the next poll. That is what lets a
//!   sign-in survive a closed browser or a service restart.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "auth-plugin",
});

use exports::rdownloader::plugin::auth::{AuthState, Guest, UserPrompt};
use rdownloader::plugin::types::{Failure, FailureKind};

struct Component;

impl Guest for Component {
    /// Starts the flow and hands back what the person has to do.
    ///
    /// Return the verification URL as the provider gave it to you. It is shown, never opened
    /// automatically, and it must be on a domain your manifest declares — the host refuses
    /// anything else rather than sending someone to an address a plugin invented.
    fn begin(_account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        Ok(AuthState::UserAction(UserPrompt {
            verification_url: "https://api.example.com/device".to_owned(),
            user_code: Some("ABCD-EFGH".to_owned()),
            expires_in_seconds: Some(600),
            // What the next poll is made with — the provider's device code, not the code the
            // person types. It is stored by the host and never shown; it is not a credential,
            // and a token must never travel here.
            flow_state: Some("device-code-from-the-provider".to_owned()),
        }))
    }

    /// Asks the provider whether the person has confirmed yet.
    ///
    /// `Pending` says how long to wait; the host does the waiting, so do not sleep here.
    /// Once the provider hands over a token, store it before returning `Authorized` — the
    /// host takes that answer to mean the credential is already kept.
    fn poll(_account_id: String, flow_state: Option<String>) -> Result<AuthState, Failure> {
        // Without what `begin` handed over there is nothing to poll with. Failing says so once
        // instead of asking the provider a question it cannot answer, for ever.
        let Some(_device_code) = flow_state.filter(|state| !state.is_empty()) else {
            return Ok(AuthState::Failed(Failure {
                category: FailureKind::AuthRequired,
                message: "the sign-in has nothing to continue with".to_owned(),
                code: Some("{{PLUGIN_SLUG}}.flow_expired".to_owned()),
                params: Vec::new(),
            }));
        };
        Ok(AuthState::Pending(5))
    }
}

export!(Component);
