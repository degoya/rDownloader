//! Reading MEGA's `us0`/`us` answers, and the shapes of the two requests.
//!
//! Pure: no WIT, no network, no `wasm32`. Everything here is exercised by the tests at the
//! bottom on the host target, which is why the parsing is here and not in `guest`.
//!
//! MEGA's command endpoint answers **every** call with HTTP 200 and puts a failure in the
//! body as a negative number, either inside the array (`[-9]`) or bare (`-9`). That is
//! measured behaviour, recorded in `plugins/mega-common/src/api.rs`, and it is why nothing
//! here reads a status code.

/// The base64 MEGA uses everywhere: URL-safe, unpadded, and forgiving about what it takes.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Account version 2, the only one this plugin signs in.
pub const ACCOUNT_VERSION_2: u64 = 2;
/// Rounds MEGA's version 2 derivation runs. Fixed by the provider, not by us.
pub const PBKDF2_ROUNDS: u32 = 100_000;
/// Bytes that derivation produces: sixteen that unwrap the master key, sixteen that are sent.
pub const DERIVED_BYTES: u32 = 32;
/// Bytes of the decrypted `csid` that make up a session identifier.
pub const SESSION_BYTES: usize = 43;
/// Longest e-mail address this plugin will put in a request.
pub const MAX_USER_BYTES: usize = 320;

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
    let version = number_field(body, "v")?;
    // A version 1 account has no salt, and this plugin refuses it anyway; answering with a
    // `Preflight` that carries an empty salt lets the caller report the version rather than
    // "unreadable answer", which is the difference between a person knowing what to do and
    // not.
    let salt = string_field(body, "s")
        .and_then(|value| b64_decode(&value))
        .unwrap_or_default();
    Some(Preflight { salt, version })
}

/// Reads the `us` answer.
#[must_use]
pub fn session(body: &str) -> Option<Session> {
    let wrapped_master_key = b64_decode(&string_field(body, "k")?)?;
    let wrapped_private_key = b64_decode(&string_field(body, "privk")?)?;
    let encrypted_session_id = b64_decode(&string_field(body, "csid")?)?;
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

/// A string field of a flat JSON object, without a JSON parser.
///
/// The answers this reads are three fields of base64 and a number. Pulling in a parser for
/// that would be a dependency in a signed component, and the sibling auth plugins all made
/// the same choice for the same reason.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let needle = format!("\"{name}\":\"");
    let start = body.find(&needle)? + needle.len();
    let end = body[start..].find('"')? + start;
    Some(body[start..end].to_owned())
}

/// A numeric field of a flat JSON object.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let needle = format!("\"{name}\":");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// MEGA's base64: URL-safe, unpadded, and tolerant of the standard alphabet and of padding.
#[must_use]
pub fn b64_decode(value: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut held = 0_u32;
    let mut out = Vec::with_capacity(value.len() * 3 / 4);
    for character in value.bytes() {
        let symbol = match character {
            b'+' => b'-',
            b'/' => b'_',
            b'=' | b'\n' | b'\r' | b' ' => continue,
            other => other,
        };
        let index = ALPHABET.iter().position(|entry| *entry == symbol)? as u32;
        held = (held << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((held >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// MEGA's base64, the other way.
#[must_use]
pub fn b64_encode(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len().div_ceil(3) * 4);
    for chunk in value.chunks(3) {
        let mut block = [0_u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        let symbols = [
            (packed >> 18) & 0x3f,
            (packed >> 12) & 0x3f,
            (packed >> 6) & 0x3f,
            packed & 0x3f,
        ];
        for symbol in symbols.iter().take(chunk.len() + 1) {
            out.push(ALPHABET[*symbol as usize] as char);
        }
    }
    out
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod flow_tests;
