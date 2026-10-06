//! What a mock token endpoint recorded, and what the host was asked to store.

use std::sync::Mutex;

use rd_core::{AccountId, Failure};
use rd_plugin_host::extension::AuthorizationRequest;

/// One request the plugin made, flattened to what a test wants to assert on.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub url: String,
    pub form: Vec<(String, String)>,
}

impl Recorded {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.form
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What `store-oauth-token` was called with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stored {
    pub account_id: AccountId,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_seconds: Option<u64>,
}

/// A mock's `store_oauth_token`: records the call and accepts it.
pub fn store(
    stored: &Mutex<Vec<Stored>>,
    account_id: AccountId,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in_seconds: Option<u64>,
) -> Result<(), Failure> {
    stored.lock().expect("stored").push(Stored {
        account_id,
        access_token: access_token.to_owned(),
        refresh_token: refresh_token.map(str::to_owned),
        expires_in_seconds,
    });
    Ok(())
}

/// One query parameter of the authorization URL.
pub fn parameter(request: &AuthorizationRequest, name: &str) -> Option<String> {
    url::Url::parse(&request.authorization_url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// base64url, unpadded, of the SHA-256 digest — PKCE's `S256`, computed independently of the
/// plugin so a matching pair is evidence rather than a tautology.
pub fn s256(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
