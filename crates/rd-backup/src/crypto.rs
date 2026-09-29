//! The key a full backup is sealed with (RD-160-01).
//!
//! The owner's decision of 2026-09-28: the passphrase is typed once, when the backup is set
//! up. The key derived from it is kept in the secret store so a scheduled run needs no input;
//! the passphrase itself is stored nowhere. A restore asks for the passphrase again and derives
//! the same key from the salt every archive carries in its header (`crate::stream`).
//!
//! The derivation is the settings bundle's (Argon2id, 19 MiB, two passes, one lane), with the
//! same parameters, so the settings bundle inside a full backup seals its secrets under this
//! very key and opens with the same passphrase through the ordinary settings import.

use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// Argon2id memory cost in KiB.
pub const KDF_M_COST: u32 = 19_456;
/// Argon2id passes.
pub const KDF_T_COST: u32 = 2;
/// Argon2id lanes.
pub const KDF_P_COST: u32 = 1;
/// Length of the Argon2id salt.
pub const SALT_LEN: usize = 16;
/// Length of the derived key.
pub const KEY_LEN: usize = 32;
/// Shortest passphrase accepted, in characters; the settings bundle's rule.
pub const MIN_PASSPHRASE_CHARS: usize = 8;

/// A derived backup key and the salt it was derived with.
///
/// Deliberately without `Clone` and with a `Debug` that prints only the fingerprint: the bytes
/// leave this type only through [`BackupKey::key_bytes`], for the secret store and the cipher.
pub struct BackupKey {
    key: Zeroizing<[u8; KEY_LEN]>,
    salt: [u8; SALT_LEN],
}

impl std::fmt::Debug for BackupKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackupKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl BackupKey {
    /// Derives a key under a fresh random salt: the setup, and every passphrase change.
    ///
    /// # Errors
    ///
    /// When the derivation cannot run.
    pub async fn derive_new(passphrase: &str) -> anyhow::Result<Self> {
        let mut salt = [0_u8; SALT_LEN];
        rand::rng().fill_bytes(&mut salt);
        Self::derive(passphrase, salt).await
    }

    /// Derives the key for `salt`: what a restore does with the salt of an archive's header.
    ///
    /// Argon2id takes a noticeable fraction of a second by design, so it runs off the runtime.
    ///
    /// # Errors
    ///
    /// When the derivation cannot run.
    pub async fn derive(passphrase: &str, salt: [u8; SALT_LEN]) -> anyhow::Result<Self> {
        let passphrase = Zeroizing::new(passphrase.to_owned());
        tokio::task::spawn_blocking(move || derive_blocking(&passphrase, salt))
            .await
            .map_err(|error| anyhow::anyhow!("join backup key derivation: {error}"))?
    }

    /// Rebuilds a key the secret store kept.
    ///
    /// # Errors
    ///
    /// When either part has the wrong length.
    pub fn from_stored(key: &[u8], salt: &[u8]) -> anyhow::Result<Self> {
        let key: [u8; KEY_LEN] = key
            .try_into()
            .map_err(|_| anyhow::anyhow!("the stored backup key has the wrong length"))?;
        let salt: [u8; SALT_LEN] = salt
            .try_into()
            .map_err(|_| anyhow::anyhow!("the stored backup salt has the wrong length"))?;
        Ok(Self {
            key: Zeroizing::new(key),
            salt,
        })
    }

    /// The key itself, for the secret store and the cipher. Never logged, never serialized.
    #[must_use]
    pub fn key_bytes(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// The salt the key was derived with; not secret, and in every archive's header.
    #[must_use]
    pub fn salt(&self) -> [u8; SALT_LEN] {
        self.salt
    }

    /// Whether `other` is the same key, compared in constant time: how a passphrase change
    /// checks the current passphrase against the stored key without a timing signal.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        bool::from(self.key.as_slice().ct_eq(other.key.as_slice())) && self.salt == other.salt
    }

    /// Whether this key has the fingerprint `fingerprint`, compared in constant time; the
    /// fallback when the stored key cannot be read back from the secret store.
    #[must_use]
    pub fn has_fingerprint(&self, fingerprint: &str) -> bool {
        bool::from(self.fingerprint().as_bytes().ct_eq(fingerprint.as_bytes()))
    }

    /// Sixteen hex characters that tell two keys apart in the interface and the audit log.
    ///
    /// A domain-separated SHA-256 of the key, cut short: it identifies a key without being a
    /// way to test passphrases faster than Argon2id allows, because it is taken of the derived
    /// key, never of the passphrase.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"rDownloader backup key fingerprint v1");
        hasher.update(self.key.as_slice());
        hex::encode(hasher.finalize())[..16].to_owned()
    }
}

fn derive_blocking(passphrase: &str, salt: [u8; SALT_LEN]) -> anyhow::Result<BackupKey> {
    let params = Params::new(KDF_M_COST, KDF_T_COST, KDF_P_COST, Some(KEY_LEN))
        .map_err(|error| anyhow::anyhow!("configure Argon2id: {error}"))?;
    let mut key = Zeroizing::new([0_u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut_slice())
        .map_err(|error| anyhow::anyhow!("derive backup key: {error}"))?;
    Ok(BackupKey { key, salt })
}

#[cfg(test)]
mod tests {
    use super::BackupKey;

    #[tokio::test]
    async fn the_same_passphrase_and_salt_give_the_same_key_and_another_salt_does_not() {
        let first = BackupKey::derive_new("correct horse")
            .await
            .expect("derive");
        let again = BackupKey::derive("correct horse", first.salt())
            .await
            .expect("derive");
        assert_eq!(first.key_bytes(), again.key_bytes());
        assert_eq!(first.fingerprint(), again.fingerprint());
        let other = BackupKey::derive_new("correct horse")
            .await
            .expect("derive");
        assert_ne!(first.salt(), other.salt());
        assert_ne!(first.key_bytes(), other.key_bytes());
        let wrong = BackupKey::derive("wrong horse", first.salt())
            .await
            .expect("derive");
        assert_ne!(first.key_bytes(), wrong.key_bytes());
    }

    #[tokio::test]
    async fn only_the_same_passphrase_under_the_same_salt_matches() {
        let key = BackupKey::derive_new("correct horse")
            .await
            .expect("derive");
        let same = BackupKey::derive("correct horse", key.salt())
            .await
            .expect("derive");
        let wrong = BackupKey::derive("wrong horse", key.salt())
            .await
            .expect("derive");
        assert!(key.matches(&same));
        assert!(!key.matches(&wrong));
        assert!(same.has_fingerprint(&key.fingerprint()));
        assert!(!wrong.has_fingerprint(&key.fingerprint()));
    }

    #[tokio::test]
    async fn a_stored_key_comes_back_whole_and_its_debug_shows_no_bytes() {
        let key = BackupKey::derive_new("correct horse")
            .await
            .expect("derive");
        let stored = BackupKey::from_stored(key.key_bytes(), &key.salt()).expect("stored");
        assert_eq!(stored.key_bytes(), key.key_bytes());
        assert!(BackupKey::from_stored(&[0; 31], &key.salt()).is_err());
        let printed = format!("{key:?}");
        assert!(printed.contains(&key.fingerprint()));
        assert!(!printed.contains(&hex::encode(key.key_bytes())));
    }
}
