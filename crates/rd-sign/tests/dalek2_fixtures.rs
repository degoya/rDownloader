//! Documents signed while this crate was on `ed25519-dalek` 2 still verify (RD-140-12).
//!
//! A frozen copy of the tool manifest as release 1.3 shipped it, not the live `resources/`
//! file: that one is re-signed at every release, and one release after the move it would prove
//! nothing about the old version any more. Only the signature layer is checked — the
//! domain-separated digest and Ed25519 verification against the compiled-in roots — not
//! freshness, which a frozen file is bound to fail once its window has passed. The site-rule
//! file that sat beside it went with the site-rule signature (RD-1230-03).

use rd_sign::{EMBEDDED_KEYS, Role, SignedDocument, TrustStore, VerifyError};

// The signing crate's own constant (`rd_tools::TOOL_MANIFEST_DOMAIN`); this crate sits below
// it. A misspelling here cannot pass unnoticed, because the domain is part of the signed digest.
const TOOL_MANIFEST_DOMAIN: &str = "rdownloader.tool-manifest.v1";
/// Any other domain: the plugin index's (`rd_plugin_host::index`).
const OTHER_DOMAIN: &str = "rdownloader.plugin-index.v1";

const TOOL_MANIFEST: &[u8] = include_bytes!("fixtures/dalek2/tools-manifest.json");

/// Every compiled-in root of `role`, regardless of its `not_after`: the question here is
/// whether the bytes verify, not whether the key is still in its window.
fn roots(role: Role) -> TrustStore {
    let store = TrustStore::new();
    for key in EMBEDDED_KEYS
        .iter()
        .filter(|key| key.role == role && !key.public_key.is_empty())
    {
        store
            .trust_base64(key.key_id.to_owned(), key.public_key)
            .expect("an embedded root decodes");
    }
    store
}

fn verify(bytes: &[u8], domain: &str, role: Role) -> Result<serde_json::Value, VerifyError> {
    SignedDocument::parse(bytes)
        .expect("the fixture parses")
        .verify(domain, &roots(role))
}

#[test]
fn a_tool_manifest_signed_by_ed25519_dalek_2_still_verifies() {
    let payload = verify(TOOL_MANIFEST, TOOL_MANIFEST_DOMAIN, Role::ToolManifest)
        .expect("the tool manifest of release 1.3 verifies");
    assert!(payload.get("tools").is_some());
}

/// The one above is not vacuous: the same signature under another domain is refused.
#[test]
fn the_same_signature_under_another_domain_does_not_verify() {
    let result = verify(TOOL_MANIFEST, OTHER_DOMAIN, Role::ToolManifest);
    assert!(matches!(result, Err(VerifyError::BadSignature { .. })));
}
