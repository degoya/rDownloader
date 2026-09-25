//! Signature, digest and trust primitives shared by everything this project verifies.
//!
//! Four features need the same handful of operations — verify an Ed25519 signature over a
//! length-prefixed digest, decide whether a key is trusted, refuse a stale document, and
//! find the compiled-in root to start from: application updates (RD-130-01/02), the managed
//! external-tool manifest (RD-102-02/03), plugin packages, and plugin repository indexes
//! (RD-140-01/02).
//!
//! All of it previously lived inside `rd-plugin-host`, which links wasmtime and every
//! bundled resolver. An updater that has to run *before* the application is known to be
//! healthy cannot afford that dependency, and duplicating the digest framing into a second
//! crate would mean two implementations of a format that must stay byte-identical forever.
//! Hence a leaf crate with no dependency beyond Ed25519, SHA-256, base64, JSON and time.
//!
//! `rd-plugin-host` re-exports what it used to own, so the packaging CLI and the shipped
//! `.rdplug` files are unaffected.

pub mod digest;
pub mod envelope;
pub mod replay;
pub mod roots;
pub mod trust;

pub use digest::{DigestBuilder, hex_sha256};
pub use envelope::{
    ALGORITHM_ED25519, DocumentSignature, SignedDocument, VerifyError, sign_detached,
    sign_document, verify_detached, verify_detached_bytes,
};
pub use replay::{Freshness, StaleError};
pub use roots::{
    EMBEDDED_KEYS, EmbeddedKey, PLUGIN_RELEASE_KEY_ID, Role, SITE_RULES_KEY_ID,
    TOOL_MANIFEST_KEY_ID, keys_for, keys_for_now, trust_store_for,
};
pub use trust::{TrustStore, decode_public_key, key_fingerprint};

pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
