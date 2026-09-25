//! The key schedule, and the two small decryptions a link's metadata needs.
//!
//! Every value here was recomputed against a public example file on 2026-09-21 and the same
//! values measured again on 2026-09-22; `crypto_tests.rs` holds them as assertions, so a
//! change to this module that would produce a different file name or a different expected
//! integrity value fails rather than downloading rubbish under a plausible name.

use aes::{
    Aes128,
    cipher::{BlockDecrypt as _, KeyInit as _, generic_array::GenericArray},
};
use base64::{
    Engine as _,
    engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD},
};

/// MEGA's alphabet is URL-safe and unpadded, but old links carry `+/` and stray `=`, and the
/// last character of a 43-character key holds bits that no byte uses. All three are accepted.
const FORGIVING: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
);

/// Decodes one of MEGA's base64 values.
#[must_use]
pub fn b64_decode(value: &str) -> Option<Vec<u8>> {
    let normalised: String = value
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '=')
        .map(|character| match character {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    FORGIVING.decode(normalised).ok()
}

/// Encodes bytes the way MEGA writes them.
#[must_use]
pub fn b64_encode(value: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

/// The three values a 32-byte file key carries.
///
/// The key itself is the exclusive-or of the halves; the counter prefix is words four and
/// five; the condensed integrity value MEGA publishes is words six and seven. A 16-byte key
/// -- what a folder's `t=1` node carries -- has none of the latter two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileKey {
    pub key: [u8; 16],
    pub nonce: [u8; 8],
    pub meta_mac: [u8; 8],
}

impl FileKey {
    /// Folds a 32-byte node key into the three values. `None` for any other length.
    #[must_use]
    pub fn from_raw(raw: &[u8]) -> Option<Self> {
        let raw: &[u8; 32] = raw.try_into().ok()?;
        let mut key = [0_u8; 16];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = raw[index] ^ raw[index + 16];
        }
        let mut nonce = [0_u8; 8];
        nonce.copy_from_slice(&raw[16..24]);
        let mut meta_mac = [0_u8; 8];
        meta_mac.copy_from_slice(&raw[24..32]);
        Some(Self {
            key,
            nonce,
            meta_mac,
        })
    }

    /// The IV every chunk MAC starts from: the counter prefix twice over.
    #[must_use]
    pub fn mac_iv(&self) -> [u8; 16] {
        let mut iv = [0_u8; 16];
        iv[..8].copy_from_slice(&self.nonce);
        iv[8..].copy_from_slice(&self.nonce);
        iv
    }
}

/// Decrypts an `at` attribute block: AES-128-CBC under a zero IV, then the `MEGA` prefix.
///
/// The block is zero-padded to a multiple of sixteen, so the trailing NULs are stripped
/// rather than treated as padding a scheme would remove.
#[must_use]
pub fn decrypt_attributes(key: &[u8; 16], blob: &[u8]) -> Option<String> {
    if blob.is_empty() || !blob.len().is_multiple_of(16) {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut previous = [0_u8; 16];
    let mut plain = Vec::with_capacity(blob.len());
    for block in blob.as_chunks::<16>().0 {
        let mut buffer = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut buffer);
        for (index, byte) in buffer.iter().enumerate() {
            plain.push(byte ^ previous[index]);
        }
        previous.copy_from_slice(block);
    }
    while plain.last() == Some(&0) {
        plain.pop();
    }
    let text = String::from_utf8(plain).ok()?;
    text.strip_prefix("MEGA").map(str::to_owned)
}

/// Decrypts a folder node's key with the share key: AES-128-ECB, block by block.
#[must_use]
pub fn decrypt_node_key(share_key: &[u8; 16], raw: &[u8]) -> Option<Vec<u8>> {
    if raw.is_empty() || !raw.len().is_multiple_of(16) {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(share_key));
    let mut plain = Vec::with_capacity(raw.len());
    for block in raw.as_chunks::<16>().0 {
        let mut buffer = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut buffer);
        plain.extend_from_slice(&buffer);
    }
    Some(plain)
}

/// The name and the fingerprint an attribute block carries, if it has them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attributes {
    pub name: Option<String>,
    /// MEGA's `c`: four sparse CRC32 values and the modification time, base64 encoded.
    pub fingerprint: Option<String>,
}

impl Attributes {
    /// Reads the JSON that sits behind the `MEGA` prefix.
    #[must_use]
    pub fn parse(decrypted: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(decrypted) else {
            return Self::default();
        };
        Self {
            name: value
                .get("n")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            fingerprint: value
                .get("c")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        }
    }

    /// The modification time the fingerprint carries, as a Unix timestamp.
    ///
    /// The fingerprint is sixteen bytes of CRC32 values, a length byte and then that many
    /// little-endian bytes of time. It is the only place MEGA keeps the file's own mtime --
    /// a node's `ts` is when it was uploaded.
    #[must_use]
    pub fn modified_at(&self) -> Option<u64> {
        let raw = b64_decode(self.fingerprint.as_deref()?)?;
        let length = usize::from(*raw.get(16)?);
        if length == 0 || length > 8 || raw.len() < 17 + length {
            return None;
        }
        let mut seconds = 0_u64;
        for (index, byte) in raw[17..17 + length].iter().enumerate() {
            seconds |= u64::from(*byte) << (index * 8);
        }
        Some(seconds)
    }
}

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod crypto_tests;
