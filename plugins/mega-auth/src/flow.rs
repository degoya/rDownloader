//! Reading MEGA's `us0`/`us` answers, and the shapes of the two requests.
//!
//! Pure: no WIT, no network, no `wasm32`. Everything here is exercised by the tests at the
//! bottom on the host target, which is why the parsing is here and not in `guest`.
//!
//! MEGA's command endpoint answers **every** call with HTTP 200 and puts a failure in the
//! body as a negative number, either inside the array (`[-9]`) or bare (`-9`). That is
//! measured behaviour, recorded in `plugins/mega-common/src/api.rs`, and it is why nothing
//! here reads a status code.

use serde_json::Value;

/// MEGA's base64 — URL-safe, unpadded, and forgiving about the standard alphabet and padding —
/// is `mega-common`'s, the one the resolver and the crawler read links with (RD-191-07).
pub use mega_common::crypto::{b64_decode, b64_encode};

/// Account version 2, the only one this plugin signs in.
pub const ACCOUNT_VERSION_2: u64 = 2;
/// Rounds MEGA's version 2 derivation runs. Fixed by the provider, not by us.
pub const PBKDF2_ROUNDS: u32 = 100_000;
/// Bytes that derivation produces: sixteen that unwrap the master key, sixteen that are sent.
pub const DERIVED_BYTES: u32 = 32;
/// Bytes of the decrypted `csid` that make up a session identifier.
pub const SESSION_BYTES: usize = 43;

/// What `us0` answered: the account's salt and which derivation it wants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preflight {
    /// The account salt, already decoded.
    pub salt: Vec<u8>,
    /// MEGA's account version. Anything but [`ACCOUNT_VERSION_2`] is refused.
    pub version: u64,
}

/// What `us` answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    /// The master key, wrapped under the first half of the derivation. One AES block.
    pub wrapped_master_key: Vec<u8>,
    /// The RSA private key block, wrapped under the master key.
    pub wrapped_private_key: Vec<u8>,
    /// The session identifier, encrypted to the account's own public key.
    pub encrypted_session_id: Vec<u8>,
}

/// The `us0` body. `{{username}}` is the host's marker: the address is substituted on the
/// way out, so this plugin never holds it either.
#[must_use]
pub fn preflight_body() -> Vec<u8> {
    br#"[{"a":"us0","user":"{{username}}"}]"#.to_vec()
}

/// The `us` body, carrying the derived half MEGA is meant to receive.
///
/// `uh` is the *second* sixteen bytes of the derivation, base64-encoded. The first sixteen
/// unwrap the master key and never leave the host.
#[must_use]
pub fn sign_in_body(user_hash: &str) -> Vec<u8> {
    format!(r#"[{{"a":"us","user":"{{{{username}}}}","uh":"{user_hash}"}}]"#).into_bytes()
}

/// The negative number MEGA answered with, if it answered with one.
#[must_use]
pub fn api_error(body: &str) -> Option<i64> {
    let trimmed = body.trim().trim_start_matches('[').trim_end_matches(']');
    let trimmed = trimmed.trim();
    if !trimmed.starts_with('-') {
        return None;
    }
    trimmed.parse::<i64>().ok()
}

/// Reads the `us0` answer.
#[must_use]
pub fn preflight(body: &str) -> Option<Preflight> {
    let answer = answer_object(body)?;
    let version = answer.get("v")?.as_u64()?;
    // A version 1 account has no salt, and this plugin refuses it anyway; answering with a
    // `Preflight` that carries an empty salt lets the caller report the version rather than
    // "unreadable answer", which is the difference between a person knowing what to do and
    // not.
    let salt = answer
        .get("s")
        .and_then(Value::as_str)
        .and_then(b64_decode)
        .unwrap_or_default();
    Some(Preflight { salt, version })
}

/// Reads the `us` answer.
#[must_use]
pub fn session(body: &str) -> Option<Session> {
    let answer = answer_object(body)?;
    let block = |name: &str| {
        answer
            .get(name)
            .and_then(Value::as_str)
            .and_then(b64_decode)
    };
    let wrapped_master_key = block("k")?;
    let wrapped_private_key = block("privk")?;
    let encrypted_session_id = block("csid")?;
    // One AES block, and a private-key block that is whole blocks. A length the derivation
    // would refuse is better caught here, where the plugin can say what is wrong.
    if wrapped_master_key.len() != 16
        || wrapped_private_key.is_empty()
        || wrapped_private_key.len() % 16 != 0
        || encrypted_session_id.is_empty()
    {
        return None;
    }
    Some(Session {
        wrapped_master_key,
        wrapped_private_key,
        encrypted_session_id,
    })
}

/// What the host stores for this account: the session identifier and the master key.
///
/// Both are needed and neither is the credential. The identifier authenticates the next
/// request; the master key unwraps the node keys of the account's own files, which is what
/// makes an account file downloadable at all. They travel as one value because `store-token`
/// takes one, in the shape the host splits (RD-120-30): `token` becomes what
/// `{{secret:mega_session}}` sends, `key` becomes key material the host only ever computes
/// with and no request ever carries.
#[must_use]
pub fn stored_session(session_id: &str, master_key: &[u8]) -> String {
    format!(
        r#"{{"token":"{session_id}","key":"{}"}}"#,
        b64_encode(master_key)
    )
}

/// The session identifier: the first [`SESSION_BYTES`] bytes of the decrypted `csid`.
#[must_use]
pub fn session_id(decrypted_csid: &[u8]) -> Option<String> {
    if decrypted_csid.len() < SESSION_BYTES {
        return None;
    }
    Some(b64_encode(&decrypted_csid[..SESSION_BYTES]))
}

/// The one object a command answer carries: MEGA wraps the answer to a batch of one in an
/// array, and a bare object is read the same way.
///
/// Parsed, not scanned (RD-191-07). The scan this replaced looked for `"name":"` and took
/// everything to the next quote, so a space after the colon or a field of the same name in a
/// nested object decided what was read — and what is read here is key material.
fn answer_object(body: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(body.trim()).ok()? {
        Value::Array(items) => match items.into_iter().next()? {
            Value::Object(object) => Some(object),
            _ => None,
        },
        Value::Object(object) => Some(object),
        _ => None,
    }
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod flow_tests;
