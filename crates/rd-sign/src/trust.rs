//! Which keys are trusted, and what has been withdrawn.
//!
//! Two axes, because they answer different questions. Revoking a **key** says "nothing this
//! author signs from now on is acceptable"; revoking a **digest** says "this exact artefact
//! is bad, whoever signed it". A key-only model cannot express the second, and the second is
//! what a published-then-withdrawn release or plugin version needs — revoking the key there
//! would take down every other artefact the same author signed, which is a far larger blast
//! radius than the problem.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock, RwLockReadGuard},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;

use crate::digest::hex_sha256;

/// Hex SHA-256 of the raw 32-byte public key — what the user is shown on first use.
///
/// The key itself is base64 and looks like any other opaque blob; a hex fingerprint is what
/// a person can compare against a published one without reading 44 characters of base64.
#[must_use]
pub fn key_fingerprint(key: &VerifyingKey) -> String {
    hex_sha256(key.as_bytes())
}

/// Decodes a base64 32-byte Ed25519 public key.
pub fn decode_public_key(encoded: &str) -> Result<VerifyingKey> {
    let raw = STANDARD
        .decode(encoded.trim())
        .context("decode public key")?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key must contain 32 bytes"))?;
    VerifyingKey::from_bytes(&bytes).context("invalid Ed25519 public key")
}

/// Trusted keys and withdrawn artefacts, shared between clones.
///
/// Cloning shares the state on purpose: a key confirmed at runtime (trust on first use) has
/// to be visible to every holder at once, or the component that happens to hold a stale copy
/// would go on refusing what the user just accepted.
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    inner: Arc<RwLock<TrustState>>,
}

#[derive(Debug, Default)]
struct TrustState {
    keys: HashMap<String, VerifyingKey>,
    revoked_digests: HashSet<[u8; 32]>,
}

impl TrustStore {
    /// An empty store: nothing is trusted until a key is added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an explicitly trusted key under `key_id`.
    pub fn trust(&self, key_id: String, key: VerifyingKey) -> Result<()> {
        if key_id.trim().is_empty() {
            bail!("key id is empty");
        }
        self.write()?.keys.insert(key_id, key);
        Ok(())
    }

    /// Adds a trusted key from its base64 encoding.
    pub fn trust_base64(&self, key_id: String, encoded: &str) -> Result<()> {
        self.trust(key_id, decode_public_key(encoded)?)
    }

    /// Drops a key, so anything newly presented under it is refused again.
    ///
    /// Returns whether the key was trusted at all, which the caller reports back to the user:
    /// "revoked" and "was never trusted" are different answers to the same request.
    pub fn revoke(&self, key_id: &str) -> Result<bool> {
        Ok(self.write()?.keys.remove(key_id).is_some())
    }

    /// The key trusted under `key_id`, if any.
    pub fn key(&self, key_id: &str) -> Result<Option<VerifyingKey>> {
        Ok(self.read()?.keys.get(key_id).copied())
    }

    /// Whether `key_id` is currently trusted.
    pub fn is_trusted(&self, key_id: &str) -> Result<bool> {
        Ok(self.read()?.keys.contains_key(key_id))
    }

    /// Every trusted key id, sorted, for a listing.
    pub fn key_ids(&self) -> Result<Vec<String>> {
        let mut ids: Vec<String> = self.read()?.keys.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }

    /// Withdraws one exact artefact by its digest, leaving its signing key trusted.
    pub fn revoke_digest(&self, digest: [u8; 32]) -> Result<()> {
        self.write()?.revoked_digests.insert(digest);
        Ok(())
    }

    /// Whether this exact artefact has been withdrawn.
    pub fn is_revoked_digest(&self, digest: &[u8; 32]) -> Result<bool> {
        Ok(self.read()?.revoked_digests.contains(digest))
    }

    fn read(&self) -> Result<RwLockReadGuard<'_, TrustState>> {
        self.inner
            .read()
            .map_err(|_| anyhow::anyhow!("trust store is poisoned"))
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, TrustState>> {
        self.inner
            .write()
            .map_err(|_| anyhow::anyhow!("trust store is poisoned"))
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;

    fn key(seed: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[seed; 32]).verifying_key()
    }

    /// Trust confirmed on one holder has to be visible to the others.
    #[test]
    fn trust_is_shared_between_clones() {
        let store = TrustStore::new();
        let clone = store.clone();
        store.trust("release".to_owned(), key(1)).expect("trust");
        assert!(clone.is_trusted("release").expect("read"));
    }

    /// A key id has to name something; an empty one would silently collect keys.
    #[test]
    fn an_empty_key_id_is_refused() {
        let store = TrustStore::new();
        assert!(store.trust("  ".to_owned(), key(1)).is_err());
    }

    /// Withdrawing one artefact must not take its author's other artefacts with it.
    #[test]
    fn revoking_a_digest_leaves_the_signing_key_trusted() {
        let store = TrustStore::new();
        store.trust("release".to_owned(), key(1)).expect("trust");
        store.revoke_digest([9; 32]).expect("revoke");
        assert!(store.is_revoked_digest(&[9; 32]).expect("read"));
        assert!(!store.is_revoked_digest(&[8; 32]).expect("read"));
        assert!(store.is_trusted("release").expect("read"));
    }

    /// "Revoked" and "was never trusted" are different answers.
    #[test]
    fn revoking_reports_whether_the_key_was_trusted() {
        let store = TrustStore::new();
        store.trust("release".to_owned(), key(1)).expect("trust");
        assert!(store.revoke("release").expect("revoke"));
        assert!(!store.revoke("release").expect("revoke"));
    }

    /// The fingerprint is what a person compares, so it must not drift.
    #[test]
    fn fingerprints_are_stable_hex_sha256() {
        let fingerprint = key_fingerprint(&key(3));
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fingerprint, key_fingerprint(&key(3)));
    }

    /// A key that is not 32 bytes is a typo or a different algorithm, never a key.
    #[test]
    fn a_public_key_of_the_wrong_length_is_refused() {
        assert!(decode_public_key("aGVsbG8=").is_err());
    }
}
