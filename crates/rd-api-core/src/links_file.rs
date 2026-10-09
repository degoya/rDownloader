//! Sealing and opening `.rdlinks` files (RD-1210-01).
//!
//! The format itself is `rd_collector::rdlinks`; this is the half that needs a key. A sealed
//! file is the settings backup's construction with its own associated data: Argon2id with the
//! parameters a full backup's key uses (`rd_backup::crypto`), a fresh salt per file, and
//! XChaCha20-Poly1305 over the packages document. Wrong passphrase and edited ciphertext fail
//! the same tag check and are refused under one code, as the settings import refuses them.
//!
//! The passphrase travels as [`Passphrase`], whose `Debug` prints nothing of it, so a request
//! that is logged whole cannot carry it into a log; nothing here writes it anywhere.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::Rng;
use rd_backup::{
    BackupKey, MIN_PASSPHRASE_CHARS,
    crypto::{KDF_M_COST, KDF_P_COST, KDF_T_COST, SALT_LEN},
};
use rd_collector::{LinksDocument, LinksFile, LinksKdf, SealedLinks};
use serde::Deserialize;

use crate::ApiError;

/// Bound into every seal, so a ciphertext of another rDownloader bundle never opens as links.
const AAD: &[u8] = b"rdownloader-links/1";
const KDF: &str = "argon2id";
const CIPHER: &str = "xchacha20poly1305";
const NONCE_LEN: usize = 24;

/// A passphrase on its way to the key derivation, and nowhere else.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Passphrase(String);

impl Passphrase {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// `None` for an empty field: a form that left it blank asked for no encryption.
    #[must_use]
    pub fn given(value: Option<Self>) -> Option<Self> {
        value.filter(|passphrase| !passphrase.0.is_empty())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Passphrase(..)")
    }
}

/// Reads a `.rdlinks` file into a document, opening it with `passphrase` when it is sealed.
///
/// # Errors
///
/// `rdlinks.too_large`, `rdlinks.file_invalid`, `rdlinks.passphrase_required` and
/// `rdlinks.passphrase_invalid`, each a `400`.
pub async fn read_links(
    content: &[u8],
    passphrase: Option<&Passphrase>,
) -> Result<LinksDocument, ApiError> {
    if content.len() > rd_collector::MAX_RDLINKS_BYTES {
        return Err(too_large());
    }
    match rd_collector::read_links_file(content).map_err(file_invalid)? {
        LinksFile::Plain(document) => Ok(document),
        LinksFile::Sealed(sealed) => {
            let Some(passphrase) = passphrase else {
                return Err(ApiError::bad_request(
                    "rdlinks.passphrase_required",
                    "The link file is encrypted; enter its passphrase to import it",
                ));
            };
            open(&sealed, passphrase).await
        }
    }
}

/// Seals `document` under `passphrase` into a `.rdlinks` file.
///
/// # Errors
///
/// `rdlinks.passphrase_too_short` for a passphrase under the settings bundle's minimum, and
/// `rdlinks.file_invalid` for a document the reader would refuse.
pub async fn seal_links(
    document: &LinksDocument,
    passphrase: &Passphrase,
) -> Result<Vec<u8>, ApiError> {
    if passphrase.expose().chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(ApiError::bad_request(
            "rdlinks.passphrase_too_short",
            "The passphrase needs at least 8 characters",
        )
        .with_param("min", MIN_PASSPHRASE_CHARS));
    }
    let plaintext = rd_collector::sealed_plaintext(document).map_err(file_invalid)?;
    let key = BackupKey::derive_new(passphrase.expose()).await?;
    let mut nonce = [0_u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new_from_slice(key.key_bytes())
        .map_err(|_| anyhow::anyhow!("invalid link file key"))?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("encrypt the link file"))?;
    let sealed = SealedLinks {
        kdf: LinksKdf {
            algorithm: KDF.to_owned(),
            m_cost: KDF_M_COST,
            t_cost: KDF_T_COST,
            p_cost: KDF_P_COST,
            salt: STANDARD.encode(key.salt()),
        },
        cipher: CIPHER.to_owned(),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    };
    Ok(rd_collector::write_sealed_file(&sealed)?)
}

async fn open(sealed: &SealedLinks, passphrase: &Passphrase) -> Result<LinksDocument, ApiError> {
    if sealed.kdf.algorithm != KDF
        || sealed.kdf.m_cost != KDF_M_COST
        || sealed.kdf.t_cost != KDF_T_COST
        || sealed.kdf.p_cost != KDF_P_COST
        || sealed.cipher != CIPHER
    {
        return Err(file_invalid(anyhow::anyhow!(
            "the file is sealed with parameters this build does not use"
        )));
    }
    let salt: [u8; SALT_LEN] = decode(&sealed.kdf.salt)?
        .try_into()
        .map_err(|_| file_invalid(anyhow::anyhow!("the salt has the wrong length")))?;
    let nonce: [u8; NONCE_LEN] = decode(&sealed.nonce)?
        .try_into()
        .map_err(|_| file_invalid(anyhow::anyhow!("the nonce has the wrong length")))?;
    let ciphertext = decode(&sealed.ciphertext)?;
    let key = BackupKey::derive(passphrase.expose(), salt).await?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.key_bytes())
        .map_err(|_| anyhow::anyhow!("invalid link file key"))?;
    let plaintext = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| {
            ApiError::bad_request(
                "rdlinks.passphrase_invalid",
                "The passphrase is wrong or the link file was changed",
            )
        })?;
    rd_collector::read_sealed_plaintext(&plaintext).map_err(file_invalid)
}

/// `400` for a link file over [`rd_collector::MAX_RDLINKS_BYTES`], read or about to be written.
#[must_use]
pub fn too_large() -> ApiError {
    let max_mib = rd_collector::MAX_RDLINKS_BYTES >> 20;
    ApiError::bad_request(
        "rdlinks.too_large",
        format!("The link file exceeds the {max_mib} MiB limit"),
    )
    .with_param("max_mib", max_mib)
}

fn decode(value: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(value)
        .map_err(|_| file_invalid(anyhow::anyhow!("a sealed member is not base64")))
}

fn file_invalid(error: anyhow::Error) -> ApiError {
    ApiError::bad_request("rdlinks.file_invalid", "The link file is not valid").with_param(
        "detail",
        format!("{error:#}").chars().take(200).collect::<String>(),
    )
}

#[cfg(test)]
#[path = "links_file_tests.rs"]
mod tests;
