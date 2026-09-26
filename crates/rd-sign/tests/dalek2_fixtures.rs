//! Documents signed while this crate was on `ed25519-dalek` 2 still verify (RD-140-12).
//!
//! Frozen copies of the site-rule file and the tool manifest as release 1.3 shipped them, not
//! the live `resources/` files: those are re-signed at every release, and one release after the
//! move they would prove nothing about the old version any more. Only the signature layer is
//! checked — the domain-separated digest and Ed25519 verification against the compiled-in
//! roots — not freshness, which a frozen file is bound to fail once its window has passed.

use rd_sign::{EMBEDDED_KEYS, Role, SignedDocument, TrustStore, VerifyError};

// The signing crates' own constants (`rd_siterules::SITE_RULES_DOMAIN`,
// `rd_tools::TOOL_MANIFEST_DOMAIN`); this crate sits below both of them. A misspelling here
// cannot pass unnoticed, because the domain is part of the signed digest.
const SITE_RULES_DOMAIN: &str = "rdownloader.site-rules.v1";
const TOOL_MANIFEST_DOMAIN: &str = "rdownloader.tool-manifest.v1";

const SITE_RULES: &[u8] = include_bytes!("fixtures/dalek2/site-rules.json");
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
fn a_site_rule_file_signed_by_ed25519_dalek_2_still_verifies() {
    let payload = verify(SITE_RULES, SITE_RULES_DOMAIN, Role::SiteRules)
        .expect("the site-rule file of release 1.3 verifies");
    assert!(payload.get("sequence").is_some());
}

#[test]
fn a_tool_manifest_signed_by_ed25519_dalek_2_still_verifies() {
    let payload = verify(TOOL_MANIFEST, TOOL_MANIFEST_DOMAIN, Role::ToolManifest)
        .expect("the tool manifest of release 1.3 verifies");
    assert!(payload.get("tools").is_some());
}

/// The two above are not vacuous: the same signature under another domain is refused.
#[test]
fn the_same_signature_under_another_domain_does_not_verify() {
    let result = verify(SITE_RULES, TOOL_MANIFEST_DOMAIN, Role::SiteRules);
    assert!(matches!(result, Err(VerifyError::BadSignature { .. })));
}
