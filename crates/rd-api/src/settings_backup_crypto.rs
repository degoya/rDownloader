use std::collections::BTreeMap;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiError;

const M_COST: u32 = 19_456;
const T_COST: u32 = 2;
const P_COST: u32 = 1;
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
            XNonce::from_slice(&nonce),
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
            XNonce::from_slice(&nonce),
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
