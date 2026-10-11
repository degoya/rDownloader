//! The two halves of a push message that need cryptography (RD-1240-13).
//!
//! - **Encryption** (RFC 8291 on RFC 8188's `aes128gcm`): every message gets a fresh P-256 key
//!   and salt; an ECDH with the browser's `p256dh` key and its `auth` secret derive the content
//!   key and nonce. One record, padding delimiter `0x02`, the sender's public key in the header.
//! - **VAPID** (RFC 8292): an ES256 JSON Web Token for the push service's origin, signed with the
//!   key pair the browser subscribed with, sent beside that key's public half.
//!
//! Everything comes from `aws-lc-rs`, the provider rustls already links; no crate of its own.

use anyhow::{Context, Result, anyhow, ensure};
use aws_lc_rs::{
    aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    agreement::{self, ECDH_P256, UnparsedPublicKey},
    hmac,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// The record size the header announces. One record carries the whole message.
const RECORD_SIZE: u32 = 4096;

/// The longest plaintext that keeps the encrypted body within the 4096 bytes every push service
/// accepts: 86 bytes of header, the padding delimiter and the 16-byte tag come on top (RFC 8291,
/// section 4).
const MAX_PLAINTEXT: usize = 3993;

/// An uncompressed P-256 point: `0x04`, then X and Y.
const POINT_LENGTH: usize = 65;

/// The browser's `auth` secret.
const AUTH_LENGTH: usize = 16;

/// How long a VAPID token stays valid; RFC 8292 allows at most a day.
const TOKEN_LIFETIME_SECONDS: i64 = 12 * 60 * 60;

/// The contact the token names (`sub`). Push services ask for one; Apple refuses a token without.
const CONTACT: &str = "https://rdownloader.net";

/// The key pair this installation signs its push messages with.
pub struct VapidKey {
    pair: EcdsaKeyPair,
}

impl std::fmt::Debug for VapidKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VapidKey")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

impl VapidKey {
    /// A new key pair, with its PKCS #8 form for the vault.
    pub fn generate() -> Result<(Self, Vec<u8>)> {
        let document = EcdsaKeyPair::generate_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .map_err(|_| anyhow!("could not generate a VAPID key"))?;
        let pkcs8 = document.as_ref().to_vec();
        Ok((Self::from_pkcs8(&pkcs8)?, pkcs8))
    }

    /// The key pair the vault holds.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)
            .map_err(|error| anyhow!("the stored VAPID key is unreadable: {error}"))?;
        Ok(Self { pair })
    }

    /// The public key as a browser's `applicationServerKey` takes it: URL-safe base64 of the
    /// uncompressed point.
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// The `Authorization` header for a push to `endpoint`: `vapid t=<token>, k=<public key>`.
    pub(crate) fn authorization(
        &self,
        endpoint: &reqwest::Url,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<String> {
        let audience = endpoint.origin().ascii_serialization();
        ensure!(audience != "null", "the push address has no origin");
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
            "aud": audience,
            "exp": now.timestamp() + TOKEN_LIFETIME_SECONDS,
            "sub": CONTACT,
        }))?);
        let signing_input = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(
                &aws_lc_rs::rand::SystemRandom::new(),
                signing_input.as_bytes(),
            )
            .map_err(|_| anyhow!("could not sign the VAPID token"))?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.as_ref()),
            self.public_key()
        ))
    }
}

/// Encrypts `plaintext` for the browser whose keys are `p256dh` and `auth` (both URL-safe
/// base64), with a fresh sender key and salt. The answer is the whole request body.
pub(crate) fn encrypt(p256dh: &str, auth: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let ua_public = decode(p256dh).context("the browser's p256dh key")?;
    let auth = decode(auth).context("the browser's auth secret")?;
    let sender = agreement::PrivateKey::generate(&ECDH_P256)
        .map_err(|_| anyhow!("could not generate a message key"))?;
    let mut salt = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| anyhow!("could not draw a salt"))?;
    encrypt_with(&sender, salt, &ua_public, &auth, plaintext)
}

/// [`encrypt`] with the sender key and the salt given, which is what the RFC's example fixes.
pub(crate) fn encrypt_with(
    sender: &agreement::PrivateKey,
    salt: [u8; 16],
    ua_public: &[u8],
    auth: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    ensure!(
        ua_public.len() == POINT_LENGTH && ua_public[0] == 4,
        "the browser's p256dh key is no uncompressed P-256 point"
    );
    ensure!(
        auth.len() == AUTH_LENGTH,
        "the browser's auth secret is not 16 bytes"
    );
    ensure!(
        plaintext.len() <= MAX_PLAINTEXT,
        "the push message is longer than {MAX_PLAINTEXT} bytes"
    );
    let sender_public = sender
        .compute_public_key()
        .map_err(|_| anyhow!("could not derive the message key's public half"))?;
    let sender_public = sender_public.as_ref();
    let shared = agreement::agree(
        sender,
        UnparsedPublicKey::new(&ECDH_P256, ua_public),
        anyhow!("the browser's p256dh key is not on the curve"),
        |secret| Ok(secret.to_vec()),
    )?;
    let ikm = hkdf(
        auth,
        &shared,
        &[b"WebPush: info\0".as_slice(), ua_public, sender_public],
        32,
    );
    let key = hkdf(
        &salt,
        &ikm,
        &[b"Content-Encoding: aes128gcm\0".as_slice()],
        16,
    );
    let nonce = hkdf(&salt, &ikm, &[b"Content-Encoding: nonce\0".as_slice()], 12);

    let mut record = Vec::with_capacity(plaintext.len() + 17);
    record.extend_from_slice(plaintext);
    // The last (and only) record's delimiter; no padding after it.
    record.push(2);
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_128_GCM, &key).map_err(|_| anyhow!("content key length"))?,
    );
    let nonce = Nonce::try_assume_unique_for_key(&nonce).map_err(|_| anyhow!("nonce length"))?;
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut record)
        .map_err(|_| anyhow!("could not encrypt the push message"))?;

    let mut body = Vec::with_capacity(21 + sender_public.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(u8::try_from(sender_public.len()).context("key id length")?);
    body.extend_from_slice(sender_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// HKDF-SHA-256 (RFC 5869) for at most one block of output, all this scheme needs.
fn hkdf(salt: &[u8], ikm: &[u8], info: &[&[u8]], length: usize) -> Vec<u8> {
    let prk = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, salt), ikm);
    let mut expand = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, prk.as_ref()));
    for part in info {
        expand.update(part);
    }
    expand.update(&[1]);
    expand.sign().as_ref()[..length].to_vec()
}

/// URL-safe base64, with or without padding: browsers hand out the unpadded form, but a client
/// that copied a key from elsewhere may have kept the `=`.
pub(crate) fn decode(value: &str) -> Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD.decode(value.trim().trim_end_matches('='))?)
}

/// Whether `signature` (ES256, fixed form) is `public_key`'s over `message`, for the tests.
#[cfg(test)]
pub(crate) fn verifies(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
        .verify(message, signature)
        .is_ok()
}
