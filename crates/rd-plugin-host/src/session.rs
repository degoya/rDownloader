//! What a sign-in may leave beside its session token: sixteen bytes of key material
//! (RD-120-30).
//!
//! A flow's `store-token` value is a string, and for every provider but one that string is a
//! token the host sends on the plugin's behalf. MEGA's sign-in produces two things of
//! different kinds: a session identifier, which is a bearer token and goes into requests, and
//! the account's master key, which must **never** go into a request and is only ever computed
//! with. Keeping both in one vault entry would put the key one parsing mistake away from
//! being sent to the provider as a session identifier.
//!
//! So a sign-in that has key material says so, in the one shape this module reads:
//!
//! ```json
//! {"token": "<sent as the flow slot's value>", "key": "<base64url, exactly 16 bytes>"}
//! ```
//!
//! and the host splits it into two vault entries: the token under `auth_flows.access_ref`,
//! where `{{secret:<flow slot>}}` finds it, and the key under `auth_flows.key_ref`, which no
//! marker, no header and no REST route reads -- only `derive`. That column is also how the
//! host knows a key's origin ([`crate::keyderive::SecretOrigin::SignIn`]): nothing a person
//! types is ever written there.
//!
//! A value that is not a JSON object is a plain token, exactly as before. A value that *is*
//! one and is not this shape is refused rather than stored as a token, because storing it as a
//! token is the mistake the split exists to prevent.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rd_core::{Failure, FailureKind};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::keyderive::SESSION_KEY_BYTES;

/// What a flow handed to `store-token`, read.
#[derive(Debug)]
pub(crate) enum FlowValue {
    /// A token and nothing else. Stored and sent whole, as every flow before RD-120-30.
    Token(String),
    /// A token to send and a key to compute with, kept apart from here on.
    Keyed {
        token: String,
        /// The key in canonical unpadded base64url, the form the vault keeps it in.
        key: Zeroizing<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    token: String,
    key: String,
}

fn invalid() -> Failure {
    // Deliberately says nothing about the value: it holds a credential either way.
    Failure::coded(
        FailureKind::Permanent,
        "plugin.store_token_session_invalid",
        "A sign-in stored a session this host cannot read".to_owned(),
    )
}

/// Reads what a flow stored.
///
/// # Errors
///
/// `plugin.store_token_session_invalid` for a JSON object that is not exactly `token` and
/// `key`, an empty token, or a key that is not sixteen bytes of base64url.
pub(crate) fn parse(value: &str) -> Result<FlowValue, Failure> {
    if !value.trim_start().starts_with('{') {
        return Ok(FlowValue::Token(value.to_owned()));
    }
    let envelope: Envelope = serde_json::from_str(value).map_err(|_| invalid())?;
    if envelope.token.trim().is_empty() {
        return Err(invalid());
    }
    let key = decode_key(&envelope.key)?;
    Ok(FlowValue::Keyed {
        token: envelope.token,
        key: Zeroizing::new(URL_SAFE_NO_PAD.encode(key.as_slice())),
    })
}

/// The key bytes a stored key text stands for.
///
/// Forgiving about padding, because MEGA's own base64 is unpadded and a plugin may well write
/// it either way; strict about the length, for the reason [`SESSION_KEY_BYTES`] gives.
///
/// # Errors
///
/// `plugin.store_token_session_invalid` for anything but sixteen bytes of base64url.
pub(crate) fn decode_key(text: &str) -> Result<Zeroizing<Vec<u8>>, Failure> {
    let key = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(text.trim().trim_end_matches('='))
            .map_err(|_| invalid())?,
    );
    if key.len() != SESSION_KEY_BYTES {
        return Err(invalid());
    }
    Ok(key)
}

/// Every string of `value` that must not surface in a log line afterwards.
///
/// The whole value, and -- for a keyed session -- each half on its own, in the spelling the
/// flow wrote it: a later log line that quotes only the token would otherwise pass the
/// redaction of the whole.
pub(crate) fn redactions(value: &str) -> Vec<String> {
    let mut out = vec![value.to_owned()];
    if let Ok(envelope) = serde_json::from_str::<Envelope>(value) {
        out.push(envelope.token);
        out.push(envelope.key);
    }
    out.retain(|entry| !entry.trim().is_empty());
    out
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
