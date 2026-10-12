//! The `/flash/addcrypted2` payload: base64 of AES-128-CBC, the key doubling as the IV.

use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7},
};

use crate::{Refusal, RefusalKind, jk::resolve_key};

/// Largest encrypted payload once decoded; its base64 may be twice as long before it is refused.
pub const MAX_CRYPTED_BYTES: usize = 8 * 1024 * 1024;

/// Reads the key out of `jk`, then decrypts `crypted` into the text that names the links.
///
/// The key comes first, as it always did in the agent: a `jk` the sandbox refuses is reported as
/// that whatever the payload holds.
pub async fn decrypt(crypted: &str, jk: &str) -> Result<String, Refusal> {
    let key = resolve_key(jk)
        .await
        .map_err(|error| Refusal::new(RefusalKind::InvalidKey, error))?;
    decrypt_with(crypted, &key)
}

/// [`decrypt`] with the key already in hand.
pub fn decrypt_with(crypted: &str, key: &[u8; 16]) -> Result<String, Refusal> {
    if crypted.len() > MAX_CRYPTED_BYTES * 2 {
        return Err(Refusal::new(
            RefusalKind::TooLarge,
            anyhow::anyhow!("CNL encrypted payload exceeds its size limit"),
        ));
    }
    let mut payload = STANDARD
        .decode(crypted.trim())
        .map_err(|error| Refusal::new(RefusalKind::InvalidPayload, error))?;
    if payload.len() > MAX_CRYPTED_BYTES {
        return Err(Refusal::new(
            RefusalKind::TooLarge,
            anyhow::anyhow!("CNL encrypted payload exceeds its size limit"),
        ));
    }
    let decrypted = Decryptor::<Aes128>::new_from_slices(key, key)
        .map_err(|error| Refusal::new(RefusalKind::InvalidKey, anyhow::Error::new(error)))?
        .decrypt_padded::<Pkcs7>(&mut payload)
        .map_err(|_| {
            Refusal::new(
                RefusalKind::InvalidPayload,
                anyhow::anyhow!("invalid CNL padding"),
            )
        })?;
    std::str::from_utf8(decrypted)
        .map(str::to_owned)
        .map_err(|error| Refusal::new(RefusalKind::InvalidPayload, error))
}
