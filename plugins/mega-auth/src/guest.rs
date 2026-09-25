//! The component: MEGA's `us0`/`us` pair, with the derivation done by the host.
//!
//! One call does the whole sign-in, so there is nothing for `poll` to continue: MEGA asks
//! nobody to visit anything and nothing is pending. `poll` therefore says so rather than
//! asking the provider a question it cannot answer.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "auth-plugin",
});

use exports::rdownloader::plugin::auth::{AuthState, Guest};
use rdownloader::plugin::{
    credentials, http,
    key_derivation::{self, Pbkdf2, SecretHandle, Span, Step},
    types::{Failure, FailureKind},
};

use crate::{flow, rsa};

struct Component;

/// The reference this plugin's manifest declares, and the only one it may name.
const SECRET: &str = "mega_password";

fn refuse(code: &str, message: impl Into<String>, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.into(),
        code: Some(format!("mega_auth.{code}")),
        params: Vec::new(),
    }
}

fn handle() -> SecretHandle {
    SecretHandle {
        reference: SECRET.to_owned(),
    }
}

/// The stage every chain begins with: MEGA's own derivation over the account's password.
fn pbkdf2(salt: &[u8]) -> Step {
    Step::Pbkdf2HmacSha512(Pbkdf2 {
        salt: salt.to_vec(),
        rounds: flow::PBKDF2_ROUNDS,
        length: flow::DERIVED_BYTES,
    })
}

/// One `a=us` call. The body carries `{{username}}`, which the host substitutes on the way
/// out; this plugin never holds the address either.
fn call(body: Vec<u8>) -> Result<String, Failure> {
    let response = http::http_request("POST", flow_endpoint(), &[], &[], &body)?;
    let text = String::from_utf8_lossy(&response.body).into_owned();
    if let Some(code) = flow::api_error(&text) {
        return Err(api_failure(code));
    }
    Ok(text)
}

/// MEGA's command endpoint. Named here rather than in `flow`, because `flow` is the half
/// with no notion of where a request goes.
const fn flow_endpoint() -> &'static str {
    "https://g.api.mega.co.nz/cs"
}

/// MEGA's negative answers, each with a code somebody can act on.
///
/// The numbers are the ones `plugins/mega-common/src/api.rs` recorded; the two that matter
/// here are `-9` (no such account) and `-26`, which is what a wrong password produces.
fn api_failure(code: i64) -> Failure {
    match code {
        -9 | -11 => refuse(
            "credentials_rejected",
            "MEGA refused this address and password",
            FailureKind::AccountInvalid,
        ),
        -26 => refuse(
            "multi_factor_required",
            "This MEGA account asks for a second factor, which this plugin cannot answer",
            FailureKind::AuthRequired,
        ),
        -16 => refuse(
            "account_blocked",
            "MEGA has blocked this account",
            FailureKind::AccountInvalid,
        ),
        -3 | -4 => refuse(
            "rate_limited",
            "MEGA is rate limiting sign-in attempts from this address",
            FailureKind::RateLimited(None),
        ),
        -18 => refuse(
            "unavailable",
            "MEGA is temporarily not answering sign-in requests",
            FailureKind::Transient(None),
        ),
        other => Failure {
            category: FailureKind::Permanent,
            message: format!("MEGA API error {other}"),
            code: Some("mega_auth.api_error".to_owned()),
            params: vec![("status".to_owned(), other.to_string())],
        },
    }
}

impl Guest for Component {
    /// The whole sign-in, in two requests and three derivations.
    fn begin(account_id: String, _credential_ref: Option<String>) -> Result<AuthState, Failure> {
        let preflight = flow::preflight(&call(flow::preflight_body())?).ok_or_else(|| {
            refuse(
                "bad_reply",
                "MEGA answered the sign-in probe with something this plugin could not read",
                FailureKind::Permanent,
            )
        })?;
        if preflight.version != flow::ACCOUNT_VERSION_2 || preflight.salt.is_empty() {
            // A version 1 account derives its key with 65 536 AES rounds over the password
            // instead, which is a stage the contract does not carry. Saying so is better
            // than a generic refusal: the person can upgrade the account at MEGA.
            return Err(refuse(
                "account_version_unsupported",
                "This MEGA account still uses the legacy key derivation",
                FailureKind::Unsupported,
            ));
        }

        // The half MEGA is meant to receive, and nothing around it.
        let user_hash = key_derivation::derive(
            &handle(),
            &[
                pbkdf2(&preflight.salt),
                Step::Take(Span {
                    offset: 16,
                    length: 16,
                }),
            ],
        )?;
        let session = flow::session(&call(flow::sign_in_body(&flow::b64_encode(&user_hash)))?)
            .ok_or_else(|| {
                refuse(
                    "bad_reply",
                    "MEGA answered the sign-in with something this plugin could not read",
                    FailureKind::Permanent,
                )
            })?;

        // The master key, unwrapped under the half that stays on the host.
        let unwrap_master = [
            pbkdf2(&preflight.salt),
            Step::Take(Span {
                offset: 0,
                length: 16,
            }),
            Step::AesEcbDecrypt(session.wrapped_master_key.clone()),
        ];
        let master_key = key_derivation::derive(&handle(), &unwrap_master)?;

        // And the private key, one stage further along the same chain. Sent as a chain
        // rather than as "decrypt this under the key you just gave me", because the master
        // key would then have had to travel back into the host as a value -- and a value the
        // guest supplies is a key the guest chose.
        let mut unwrap_private = unwrap_master.to_vec();
        unwrap_private.push(Step::AesEcbDecrypt(session.wrapped_private_key.clone()));
        let private_key_block = key_derivation::derive(&handle(), &unwrap_private)?;

        let session_id = rsa::PrivateKey::parse(&private_key_block)
            .and_then(|key| key.decrypt(&session.encrypted_session_id))
            .and_then(|plain| flow::session_id(&plain))
            .ok_or_else(|| {
                refuse(
                    "session_unreadable",
                    "MEGA's session identifier could not be opened with this account's key",
                    FailureKind::AuthRequired,
                )
            })?;

        // Stored before `Authorized` is returned: the host takes that answer to mean the
        // credential is already kept, so saying it first would be a lie.
        credentials::store_token(&account_id, &flow::stored_session(&session_id, &master_key))?;
        Ok(AuthState::Authorized)
    }

    /// Nothing to continue. MEGA's sign-in finishes inside `begin` or not at all.
    fn poll(_account_id: String, _flow_state: Option<String>) -> Result<AuthState, Failure> {
        Ok(AuthState::Failed(refuse(
            "nothing_to_poll",
            "a MEGA sign-in finishes in one step; start it again",
            FailureKind::AuthRequired,
        )))
    }
}

export!(Component);
