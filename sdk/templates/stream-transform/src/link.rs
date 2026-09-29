//! The address this plugin claims, and the key its fragment carries.
//!
//! Plain Rust with no dependencies, so it compiles for `wasm32-unknown-unknown` without WASI and
//! is unit-tested on the host target as it is. Replace the address shape and the key layout
//! with your provider's, and keep the two rules they encode: claiming an address never needs
//! the key, and the key never goes into a request.

use std::fmt;

/// The host the files are shared on. It must also be in `secret_fragment_domains` in
/// `manifest.toml`, or intake strips the fragment and the key is gone before `resolve` runs.
pub const FILE_HOST: &str = "files.example.com";

/// Where the provider answers questions about a file. On a domain `manifest.toml` grants.
pub const API: &str = "https://api.example.com/v1/files/";

/// Bytes of AES-128 key, then bytes of counter prefix, in the fragment.
pub const KEY_BYTES: usize = 16;
pub const NONCE_BYTES: usize = 8;

/// The key material a link carries.
///
/// `Debug` prints a placeholder rather than the bytes, so a stray `{:?}` in a log line cannot
/// leak it — the same rule the host keeps for the type it vaults this in.
#[derive(Eq, PartialEq)]
pub struct Secret {
    pub key: [u8; KEY_BYTES],
    pub nonce: [u8; NONCE_BYTES],
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

/// The file id of an address this plugin claims, or `None`.
///
/// Reads the path alone. `claims-url` may be asked about an address whose fragment intake has
/// already put away, and a claim that depended on the key would then drop the link.
#[must_use]
pub fn file_id(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")?
        .strip_prefix(FILE_HOST)?
        .strip_prefix("/f/")?;
    let id = rest.split('#').next().unwrap_or_default();
    let plausible = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    plausible.then_some(id)
}

/// The key and counter prefix the fragment carries: 24 bytes, base64url without padding.
///
/// `None` for an address without a fragment or with one of the wrong length. A key that is
/// almost right is not a key: the download would succeed and produce rubbish.
#[must_use]
pub fn secret(url: &str) -> Option<Secret> {
    let (_, fragment) = url.split_once('#')?;
    let bytes = base64_url_decode(fragment)?;
    if bytes.len() != KEY_BYTES + NONCE_BYTES {
        return None;
    }
    let mut key = [0; KEY_BYTES];
    key.copy_from_slice(&bytes[..KEY_BYTES]);
    let mut nonce = [0; NONCE_BYTES];
    nonce.copy_from_slice(&bytes[KEY_BYTES..]);
    Some(Secret { key, nonce })
}

/// Where the provider is asked about a file. Built from the id alone: the fragment is the one
/// part of the address the provider must never see.
#[must_use]
pub fn api_url(id: &str) -> String {
    format!("{API}{id}")
}

/// Base64url without padding, as link fragments spell it. Written out because a guest has no
/// WASI and a dependency for twenty lines is not worth its size.
fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{api_url, file_id, secret};

    /// Bytes 0 to 23: a key of 0..16 and a counter prefix of 16..24.
    const LINK: &str = "https://files.example.com/f/AbC-12_x#AAECAwQFBgcICQoLDA0ODxAREhMUFRYX";

    #[test]
    fn a_link_is_claimed_by_its_path_with_or_without_the_key() {
        assert_eq!(file_id(LINK), Some("AbC-12_x"));
        assert_eq!(
            file_id("https://files.example.com/f/AbC-12_x"),
            Some("AbC-12_x")
        );
        assert_eq!(file_id("https://files.example.com/f/"), None);
        assert_eq!(file_id("https://files.example.com/f/a/b"), None);
        assert_eq!(file_id("https://elsewhere.example.org/f/AbC"), None);
        assert_eq!(file_id("http://files.example.com/f/AbC"), None);
    }

    #[test]
    fn the_fragment_is_the_key_then_the_counter_prefix() {
        let secret = secret(LINK).expect("a key");
        assert_eq!(secret.key.to_vec(), (0..16).collect::<Vec<u8>>());
        assert_eq!(secret.nonce.to_vec(), (16..24).collect::<Vec<u8>>());
    }

    #[test]
    fn a_missing_or_malformed_key_is_no_key_at_all() {
        assert!(secret("https://files.example.com/f/AbC").is_none());
        assert!(secret("https://files.example.com/f/AbC#").is_none());
        // One character short, and one that is not base64url.
        assert!(
            secret("https://files.example.com/f/AbC#AAECAwQFBgcICQoLDA0ODxAREhMUFRY").is_none()
        );
        assert!(
            secret("https://files.example.com/f/AbC#AAECAwQFBgcICQoLDA0ODxAREhMUFRY+").is_none()
        );
    }

    #[test]
    fn the_request_to_the_provider_never_carries_the_key() {
        let id = file_id(LINK).expect("claimed");
        let request = api_url(id);
        assert_eq!(request, "https://api.example.com/v1/files/AbC-12_x");
        assert!(!request.contains('#'));
    }

    #[test]
    fn debug_output_does_not_print_the_key() {
        let printed = format!("{:?}", secret(LINK).expect("a key"));
        assert_eq!(printed, "Secret(..)");
    }
}
