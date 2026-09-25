//! One signed-document format, used by the update manifest, the tool manifest, the
//! compatibility rules and the plugin repository index.
//!
//! Two decisions carry the format.
//!
//! **The payload is signed as the bytes it arrived as**, kept verbatim in a `RawValue`,
//! never re-serialised. Canonicalising JSON before hashing is a well-known source of
//! signature bypasses — number formatting, key order, escaping and duplicate keys all give a
//! producer and a verifier room to disagree about what "the same document" is. Signing the
//! received bytes removes the question.
//!
//! **Every signature is bound to a domain string.** The digest covers a caller-supplied
//! label like `rdownloader.update-manifest.v1` before the payload, so a signature over an
//! update manifest cannot be lifted onto a repository index that happens to parse. Without
//! it, one format shared by four features would mean any of the four could impersonate
//! another under the same key.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{digest::DigestBuilder, trust::TrustStore};

/// The only signature algorithm this format admits.
pub const ALGORITHM_ED25519: &str = "ed25519";

/// One detached signature over a document's payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DocumentSignature {
    /// Which trusted key is claimed. Never the key itself: a document that carried its own
    /// key would verify against itself and prove nothing.
    pub key_id: String,
    /// Always [`ALGORITHM_ED25519`] today; present so a future algorithm is a rejected
    /// value rather than a silently misread signature.
    pub algorithm: String,
    /// Base64 of the raw 64-byte signature.
    pub signature: String,
}

/// A payload plus the signatures over it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignedDocument {
    payload: Box<RawValue>,
    signatures: Vec<DocumentSignature>,
}

/// Why a document was not accepted.
///
/// Separate variants because the answers differ: an unknown key is a trust decision for the
/// user, a bad signature is tampering or corruption, and a revoked digest is a withdrawal
/// the user cannot override.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// No signature named a key this installation trusts.
    #[error("no signature from a trusted key (saw: {key_ids})")]
    UntrustedKey { key_ids: String },
    /// A signature named a trusted key but did not verify.
    #[error("signature from {key_id} does not verify")]
    BadSignature { key_id: String },
    /// This exact document was withdrawn.
    #[error("this document has been revoked")]
    Revoked,
    /// Malformed input.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl SignedDocument {
    /// Parses a document without verifying anything.
    ///
    /// Deliberately separate from verification so no caller can accidentally read a payload
    /// it has not checked: [`payload`](Self::payload) is the only accessor, and it is
    /// private to this module's tests.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("parse signed document")
    }

    /// The digest this document's signatures are taken over.
    #[must_use]
    pub fn digest(&self, domain: &str) -> [u8; 32] {
        let mut builder = DigestBuilder::new();
        builder.field(domain.as_bytes());
        builder.field(self.payload.get().as_bytes());
        builder.finish()
    }

    /// Verifies against `trust` and returns the parsed payload.
    ///
    /// One valid signature from a trusted key is enough. A threshold above one belongs to a
    /// multi-signer root, which this project does not have and should not pretend to.
    pub fn verify<T: DeserializeOwned>(
        &self,
        domain: &str,
        trust: &TrustStore,
    ) -> Result<T, VerifyError> {
        let digest = self.digest(domain);
        if trust.is_revoked_digest(&digest)? {
            return Err(VerifyError::Revoked);
        }
        let mut named_a_trusted_key = None;
        for entry in &self.signatures {
            if entry.algorithm != ALGORITHM_ED25519 {
                continue;
            }
            let Some(key) = trust.key(&entry.key_id)? else {
                continue;
            };
            named_a_trusted_key = Some(entry.key_id.clone());
            if verify_detached(&key, &digest, &entry.signature).is_ok() {
                return serde_json::from_str(self.payload.get())
                    .context("parse signed document payload")
                    .map_err(VerifyError::Other);
            }
        }
        match named_a_trusted_key {
            // A trusted key was named and the signature did not hold: that is tampering or
            // corruption, and saying "untrusted key" would send the reader down the wrong path.
            Some(key_id) => Err(VerifyError::BadSignature { key_id }),
            None => Err(VerifyError::UntrustedKey {
                key_ids: self
                    .signatures
                    .iter()
                    .map(|entry| entry.key_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
        }
    }

    /// Builds a document from an already-serialised payload and its signatures.
    pub fn from_parts(payload: Box<RawValue>, signatures: Vec<DocumentSignature>) -> Self {
        Self {
            payload,
            signatures,
        }
    }
}

/// Verifies one base64 Ed25519 signature over `digest`.
pub fn verify_detached(key: &VerifyingKey, digest: &[u8; 32], encoded: &str) -> Result<()> {
    let raw = STANDARD
        .decode(encoded.trim())
        .context("decode signature")?;
    let signature = Signature::from_slice(&raw).context("invalid Ed25519 signature")?;
    key.verify(digest, &signature)
        .context("signature verification failed")
}

/// Verifies a base64 signature supplied as bytes, as an archive member holds it.
pub fn verify_detached_bytes(key: &VerifyingKey, digest: &[u8; 32], encoded: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(encoded).context("signature is not UTF-8")?;
    verify_detached(key, digest, text)
}

/// Signs `digest` and returns the base64 encoding this format stores.
pub fn sign_detached(key: &ed25519_dalek::SigningKey, digest: &[u8; 32]) -> String {
    use ed25519_dalek::Signer;
    STANDARD.encode(key.sign(digest).to_bytes())
}

/// Wraps `payload` in a document signed by `key` under `key_id` for `domain`.
pub fn sign_document<T: Serialize>(
    domain: &str,
    key_id: &str,
    key: &ed25519_dalek::SigningKey,
    payload: &T,
) -> Result<SignedDocument> {
    let serialised = serde_json::to_string(payload).context("serialise document payload")?;
    let raw = RawValue::from_string(serialised).context("payload is not a JSON value")?;
    let mut document = SignedDocument::from_parts(raw, Vec::new());
    let digest = document.digest(domain);
    document.signatures.push(DocumentSignature {
        key_id: key_id.to_owned(),
        algorithm: ALGORITHM_ED25519.to_owned(),
        signature: sign_detached(key, &digest),
    });
    if document.signatures.is_empty() {
        bail!("document was not signed");
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use serde::{Deserialize, Serialize};

    use super::*;

    const DOMAIN: &str = "rdownloader.test-document.v1";

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Payload {
        channel: String,
        version: String,
    }

    fn payload() -> Payload {
        Payload {
            channel: "stable".to_owned(),
            version: "1.0.0".to_owned(),
        }
    }

    fn signed(key: &SigningKey, key_id: &str) -> (SignedDocument, TrustStore) {
        let document = sign_document(DOMAIN, key_id, key, &payload()).expect("sign");
        let trust = TrustStore::new();
        trust
            .trust(key_id.to_owned(), key.verifying_key())
            .expect("trust");
        (document, trust)
    }

    #[test]
    fn a_signed_document_round_trips_through_json() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (document, trust) = signed(&key, "release");
        let encoded = serde_json::to_vec(&document).expect("encode");
        let parsed = SignedDocument::parse(&encoded).expect("parse");
        let read: Payload = parsed.verify(DOMAIN, &trust).expect("verify");
        assert_eq!(read, payload());
    }

    /// The whole point of the length-prefixed domain field.
    #[test]
    fn a_signature_does_not_carry_over_to_another_document_kind() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (document, trust) = signed(&key, "release");
        let result: Result<Payload, _> = document.verify("rdownloader.repository-index.v1", &trust);
        assert!(matches!(result, Err(VerifyError::BadSignature { .. })));
    }

    /// A changed payload must not verify, however small the change.
    #[test]
    fn tampering_with_the_payload_is_detected() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (document, trust) = signed(&key, "release");
        let mut encoded: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&document).expect("encode")).expect("json");
        encoded["payload"]["version"] = serde_json::Value::String("9.9.9".to_owned());
        let tampered =
            SignedDocument::parse(&serde_json::to_vec(&encoded).expect("encode")).expect("parse");
        let result: Result<Payload, _> = tampered.verify(DOMAIN, &trust);
        assert!(matches!(result, Err(VerifyError::BadSignature { .. })));
    }

    /// An unknown signer is a trust decision, not a corruption report.
    #[test]
    fn an_unknown_key_is_reported_separately_from_a_broken_signature() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (document, _) = signed(&key, "release");
        let empty = TrustStore::new();
        let result: Result<Payload, _> = document.verify(DOMAIN, &empty);
        assert!(matches!(result, Err(VerifyError::UntrustedKey { .. })));
    }

    /// A withdrawn document stays refused even though its key is still good.
    #[test]
    fn a_revoked_digest_is_refused_before_the_signature_is_looked_at() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (document, trust) = signed(&key, "release");
        trust
            .revoke_digest(document.digest(DOMAIN))
            .expect("revoke");
        let result: Result<Payload, _> = document.verify(DOMAIN, &trust);
        assert!(matches!(result, Err(VerifyError::Revoked)));
    }

    /// An unknown algorithm must be ignored rather than read as Ed25519.
    #[test]
    fn a_signature_in_an_unknown_algorithm_does_not_count() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let (mut document, trust) = signed(&key, "release");
        document.signatures[0].algorithm = "rsa".to_owned();
        let result: Result<Payload, _> = document.verify(DOMAIN, &trust);
        assert!(matches!(result, Err(VerifyError::UntrustedKey { .. })));
    }
}
