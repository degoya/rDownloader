use std::collections::BTreeMap;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiError;

// One set of Argon2id parameters for both bundles: a full backup seals its settings bundle
// under the backup key, which the same passphrase derives with the same parameters (RD-160-01).
use rd_backup::crypto::{KDF_M_COST as M_COST, KDF_P_COST as P_COST, KDF_T_COST as T_COST};

const AAD: &[u8] = b"rDownloader settings bundle v1";

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SecretKdf {
    pub algorithm: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct EncryptedSecrets {
    pub kdf: SecretKdf,
    pub cipher: String,
    pub nonce: String,
    pub ciphertext: String,
}

pub async fn encrypt_secrets(
    passphrase: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<EncryptedSecrets, ApiError> {
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 24];
    rand::rng().fill_bytes(&mut salt);
    rand::rng().fill_bytes(&mut nonce);
    let key = derive_key(passphrase.to_owned(), salt.to_vec()).await?;
    let plaintext = serde_json::to_vec(secrets).map_err(anyhow::Error::new)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| anyhow::anyhow!("invalid settings backup key"))?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("encrypt settings backup secrets"))?;
    Ok(EncryptedSecrets {
        kdf: SecretKdf {
            algorithm: "argon2id".to_owned(),
            m_cost: M_COST,
            t_cost: T_COST,
            p_cost: P_COST,
            salt: STANDARD.encode(salt),
        },
        cipher: "xchacha20poly1305".to_owned(),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

/// Seals the secret map under a full backup's key instead of a fresh derivation (RD-160-01).
///
/// The bundle records the key's salt as its own, with the same parameters, so
/// [`decrypt_secrets`] opens it with the passphrase the backup key was derived from — a restore
/// asks for the passphrase again and needs nothing else.
pub fn encrypt_secrets_with_key(
    key: &rd_backup::BackupKey,
    secrets: &BTreeMap<String, String>,
) -> Result<EncryptedSecrets, ApiError> {
    let mut nonce = [0_u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let plaintext = serde_json::to_vec(secrets).map_err(anyhow::Error::new)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.key_bytes())
        .map_err(|_| anyhow::anyhow!("invalid settings backup key"))?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("encrypt settings backup secrets"))?;
    Ok(EncryptedSecrets {
        kdf: SecretKdf {
            algorithm: "argon2id".to_owned(),
            m_cost: M_COST,
            t_cost: T_COST,
            p_cost: P_COST,
            salt: STANDARD.encode(key.salt()),
        },
        cipher: "xchacha20poly1305".to_owned(),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

pub async fn decrypt_secrets(
    passphrase: &str,
    encrypted: &EncryptedSecrets,
) -> Result<BTreeMap<String, String>, ApiError> {
    if encrypted.kdf.algorithm != "argon2id"
        || encrypted.kdf.m_cost != M_COST
        || encrypted.kdf.t_cost != T_COST
        || encrypted.kdf.p_cost != P_COST
        || encrypted.cipher != "xchacha20poly1305"
    {
        return Err(invalid_bundle("Unsupported secret encryption parameters"));
    }
    let salt = decode_sized(&encrypted.kdf.salt, 16, "salt")?;
    let nonce = decode_sized(&encrypted.nonce, 24, "nonce")?;
    let ciphertext = STANDARD
        .decode(&encrypted.ciphertext)
        .map_err(|_| invalid_bundle("Invalid encrypted secret ciphertext"))?;
    let key = derive_key(passphrase.to_owned(), salt).await?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| anyhow::anyhow!("invalid settings backup key"))?;
    let plaintext = cipher
        .decrypt(
            <&XNonce>::try_from(nonce.as_slice())
                .map_err(|_| invalid_bundle("Invalid secret encryption nonce length"))?,
            Payload {
                msg: &ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| {
            ApiError::bad_request(
                "settings.backup_passphrase_invalid",
                "The backup passphrase is invalid or the encrypted data was modified",
            )
        })?;
    serde_json::from_slice(&plaintext).map_err(|_| invalid_bundle("Invalid decrypted secret map"))
}

async fn derive_key(passphrase: String, salt: Vec<u8>) -> Result<[u8; 32], ApiError> {
    tokio::task::spawn_blocking(move || {
        let params = Params::new(M_COST, T_COST, P_COST, Some(32))
            .map_err(|error| anyhow::anyhow!("configure Argon2id: {error}"))?;
        let mut key = [0_u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.as_bytes(), &salt, &mut key)
            .map_err(|error| anyhow::anyhow!("derive settings backup key: {error}"))?;
        Ok::<_, anyhow::Error>(key)
    })
    .await
    .map_err(|error| anyhow::anyhow!("join settings backup key derivation: {error}"))?
    .map_err(Into::into)
}

fn decode_sized(value: &str, expected: usize, label: &str) -> Result<Vec<u8>, ApiError> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| invalid_bundle(format!("Invalid secret encryption {label}")))?;
    if bytes.len() != expected {
        return Err(invalid_bundle(format!(
            "Invalid secret encryption {label} length"
        )));
    }
    Ok(bytes)
}

fn invalid_bundle(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("settings.backup_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use base64::{Engine, engine::general_purpose::STANDARD};

    use super::{decrypt_secrets, encrypt_secrets};

    fn values() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("s0".to_owned(), "account password".to_owned()),
            ("s1".to_owned(), "cookie=value".to_owned()),
        ])
    }

    #[tokio::test]
    async fn encrypted_secret_map_round_trips() {
        let encrypted = encrypt_secrets("correct horse", &values())
            .await
            .expect("encrypt");
        assert_eq!(
            decrypt_secrets("correct horse", &encrypted)
                .await
                .expect("decrypt"),
            values()
        );
    }

    /// RD-160-01: the bundle inside a full backup opens with the backup's passphrase through
    /// the ordinary import path.
    #[tokio::test]
    async fn a_map_sealed_under_the_backup_key_opens_with_its_passphrase() {
        let key = rd_backup::BackupKey::derive_new("correct horse")
            .await
            .expect("key");
        let encrypted = super::encrypt_secrets_with_key(&key, &values()).expect("encrypt");
        assert_eq!(
            decrypt_secrets("correct horse", &encrypted)
                .await
                .expect("decrypt"),
            values()
        );
        let error = decrypt_secrets("wrong horse", &encrypted)
            .await
            .expect_err("wrong passphrase");
        assert_eq!(error.code(), "settings.backup_passphrase_invalid");
    }

    /// RD-190-04: the archive passwords travel in this map. One sealed by another backup's key
    /// does not open with this backup's passphrase, and a modified one does not open at all, so
    /// a restore refuses either instead of minting what it cannot vouch for.
    #[tokio::test]
    async fn a_foreign_or_tampered_backup_seal_is_refused() {
        let ours = rd_backup::BackupKey::derive_new("correct horse")
            .await
            .expect("key");
        let foreign = rd_backup::BackupKey::derive_new("correct horse")
            .await
            .expect("another backup's key");
        let mut values = values();
        values.insert("s2".to_owned(), "archive password".to_owned());
        let mut sealed = super::encrypt_secrets_with_key(&foreign, &values).expect("encrypt");
        // The foreign map under this backup's salt: the passphrase is right, the key is not.
        sealed.kdf.salt = STANDARD.encode(ours.salt());
        let error = decrypt_secrets("correct horse", &sealed)
            .await
            .expect_err("foreign seal");
        assert_eq!(error.code(), "settings.backup_passphrase_invalid");

        let mut sealed = super::encrypt_secrets_with_key(&ours, &values).expect("encrypt");
        let mut ciphertext = STANDARD.decode(&sealed.ciphertext).expect("ciphertext");
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0x01;
        sealed.ciphertext = STANDARD.encode(ciphertext);
        let error = decrypt_secrets("correct horse", &sealed)
            .await
            .expect_err("tampered seal");
        assert_eq!(error.code(), "settings.backup_passphrase_invalid");
    }

    #[tokio::test]
    async fn wrong_passphrase_is_rejected() {
        let encrypted = encrypt_secrets("correct horse", &values())
            .await
            .expect("encrypt");
        let error = decrypt_secrets("wrong horse", &encrypted)
            .await
            .expect_err("wrong passphrase");
        assert_eq!(error.code(), "settings.backup_passphrase_invalid");
    }

    #[tokio::test]
    async fn tampered_ciphertext_is_rejected() {
        let mut encrypted = encrypt_secrets("correct horse", &values())
            .await
            .expect("encrypt");
        let mut ciphertext = STANDARD.decode(&encrypted.ciphertext).expect("ciphertext");
        ciphertext[0] ^= 0x80;
        encrypted.ciphertext = STANDARD.encode(ciphertext);
        let error = decrypt_secrets("correct horse", &encrypted)
            .await
            .expect_err("tampered ciphertext");
        assert_eq!(error.code(), "settings.backup_passphrase_invalid");
    }
}
