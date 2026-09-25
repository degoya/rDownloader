//! The component: one request, one answer about one key.
//!
//! `begin` and `poll` are the same call on purpose. A guest is instantiated fresh for every
//! invocation and remembers nothing of its own, and this check has nothing to remember: it
//! reads a credential the person already stored. That is what makes a sign-in interrupted by
//! a restart finish afterwards with no `flow-state` at all.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "auth-plugin",
});

use exports::rdownloader::plugin::auth::{AuthState, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::flow::{self, Outcome};

struct Component;

/// The account endpoint, and the only address this plugin reaches.
const ACCOUNT_ENDPOINT: &str = "https://api.torbox.app/v1/api/user/me";

/// The vault reference the TorBox provider keeps its API key under.
const TOKEN_REFERENCE: &str = "torbox_api_key";

fn refuse(code: &str, message: &str, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// Asks TorBox about the key this account holds.
fn check(account_id: &str) -> Result<AuthState, Failure> {
    // Without a key there is nothing to check, and the person can act on being told so.
    // Answered before the request, because asking TorBox about a key nobody stored would spend
    // the account's request budget to learn what this installation already knows.
    if !host::secret_available(account_id, TOKEN_REFERENCE) {
        return Ok(AuthState::Failed(refuse(
            crate::KEY_MISSING,
            "this TorBox account has no API key",
            FailureKind::AuthRequired,
        )));
    }
    let response = http::http_request(
        "GET",
        ACCOUNT_ENDPOINT,
        &[],
        &[
            RequestHeader {
                name: "Authorization".to_owned(),
                // The key never enters the plugin: the host expands the marker on the way out,
                // towards `api.torbox.app` and nowhere else.
                value_template: format!("Bearer {{{{secret:{TOKEN_REFERENCE}}}}}"),
            },
            RequestHeader {
                name: "Accept".to_owned(),
                value_template: "application/json".to_owned(),
            },
        ],
        &[],
    )?;
    let retry_after = flow::retry_after_seconds(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.as_str()),
    );
    Ok(
        match flow::read(response.status, retry_after, &response.body) {
            // Nothing is stored. The credential this confirms is the one the person typed, and
            // writing it back would be overwriting their value with a copy of itself.
            Outcome::Valid => AuthState::Authorized,
            Outcome::Invalid(code) => AuthState::Failed(refuse(
                code,
                "TorBox did not accept this API key",
                FailureKind::AuthRequired,
            )),
            Outcome::Retry(seconds) => AuthState::Pending(seconds),
        },
    )
}

impl Guest for Component {
    fn begin(account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        check(&account_id)
    }

    /// The same call. There is no bookkeeping to hand back, so `flow-state` is never written
    /// and never read: a check of a stored credential is the same question every time.
    fn poll(account_id: String, _flow_state: Option<String>) -> Result<AuthState, Failure> {
        check(&account_id)
    }
}

export!(Component);
