//! Tests for [`super`]: each refusal in the documented order, with its stable code.

use chrono::Duration;
use rd_sign::{SigningKey, TrustStore};

use super::*;
use crate::format::tests::example;

const KEY_ID: &str = "test-siterules";

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).expect("timestamp")
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn trust() -> TrustStore {
    let store = TrustStore::new();
    store
        .trust(KEY_ID.to_owned(), key().verifying_key())
        .expect("trust");
    store
}

fn pack() -> RulePack {
    RulePack {
        format_version: FORMAT_VERSION,
        sequence: 3,
        issued_at: now() - Duration::hours(1),
        not_after: None,
        rules: vec![example()],
    }
}

fn signed(pack: &RulePack) -> Vec<u8> {
    sign(KEY_ID, &key(), pack).expect("sign")
}

/// Signs a payload the [`sign`] helper would refuse, so a refusal can be tested end to end.
fn signed_raw(domain: &str, payload: &serde_json::Value) -> Vec<u8> {
    let document = rd_sign::sign_document(domain, KEY_ID, &key(), payload).expect("sign");
    serde_json::to_vec(&document).expect("encode")
}

#[test]
fn a_signed_pack_verifies_and_yields_its_rules() {
    let verified = verify_with(&signed(&pack()), &trust(), None, now()).expect("verify");
    assert_eq!(verified.sequence, 3);
    assert_eq!(verified.rules.len(), 1);
    assert_eq!(verified.rules[0].id, "scnlog");
}

/// The acceptance criterion for `Role::SiteRules`: a signature made for the rule pack does
/// not hold for another document kind, and one made for another kind does not hold here.
#[test]
fn a_signature_does_not_cross_document_kinds() {
    let payload = serde_json::to_value(pack()).expect("encode");
    let tool_manifest_domain = "rdownloader.tool-manifest.v1";
    let as_tool_manifest = signed_raw(tool_manifest_domain, &payload);
    let refused = verify_with(&as_tool_manifest, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.bad_signature");

    let document = SignedDocument::parse(&signed(&pack())).expect("parse");
    let as_other: Result<serde_json::Value, _> = document.verify(tool_manifest_domain, &trust());
    assert!(matches!(as_other, Err(VerifyError::BadSignature { .. })));
}

#[test]
fn an_unknown_format_version_refuses_the_whole_pack_with_a_stable_code() {
    let mut payload = serde_json::to_value(pack()).expect("encode");
    payload["format_version"] = serde_json::json!(2);
    let bytes = signed_raw(SITE_RULES_DOMAIN, &payload);
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.format_version_unsupported");
    assert!(matches!(refused, PackError::FormatVersion { saw: 2 }));
    // The rules inside were valid; none of them is returned.
    assert!(refused.to_string().contains("format version 2"));
}

#[test]
fn a_missing_format_version_is_malformed() {
    let mut payload = serde_json::to_value(pack()).expect("encode");
    payload
        .as_object_mut()
        .expect("object")
        .remove("format_version");
    let bytes = signed_raw(SITE_RULES_DOMAIN, &payload);
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.malformed");
}

#[test]
fn an_unknown_pack_field_is_malformed() {
    let mut payload = serde_json::to_value(pack()).expect("encode");
    payload["publisher"] = serde_json::json!("x");
    let bytes = signed_raw(SITE_RULES_DOMAIN, &payload);
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.malformed");
}

/// The acceptance criterion for revocation: the existing trust store's digest axis.
#[test]
fn a_revoked_pack_is_refused() {
    let bytes = signed(&pack());
    let trust = trust();
    let document = SignedDocument::parse(&bytes).expect("parse");
    trust
        .revoke_digest(document.digest(SITE_RULES_DOMAIN))
        .expect("revoke");
    let refused = verify_with(&bytes, &trust, None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.revoked");
    assert!(matches!(refused, PackError::Revoked));
}

#[test]
fn a_revoked_key_is_untrusted() {
    let bytes = signed(&pack());
    let trust = trust();
    trust.revoke(KEY_ID).expect("revoke");
    let refused = verify_with(&bytes, &trust, None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.untrusted");
}

#[test]
fn a_tampered_pack_is_a_bad_signature() {
    let mut encoded: serde_json::Value = serde_json::from_slice(&signed(&pack())).expect("json");
    encoded["payload"]["sequence"] = serde_json::json!(99);
    let bytes = serde_json::to_vec(&encoded).expect("encode");
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.bad_signature");
}

#[test]
fn a_replayed_or_expired_pack_is_stale() {
    let bytes = signed(&pack());
    let refused = verify_with(&bytes, &trust(), Some(3), now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.stale");
    let mut expired = pack();
    expired.not_after = Some(now() - Duration::minutes(1));
    let refused = verify_with(&signed(&expired), &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.stale");
}

#[test]
fn an_invalid_rule_names_itself_and_refuses_the_pack() {
    let mut payload = serde_json::to_value(pack()).expect("encode");
    payload["rules"][0]["steps"] = serde_json::json!([]);
    let bytes = signed_raw(SITE_RULES_DOMAIN, &payload);
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.invalid_rule");
    assert!(matches!(refused, PackError::Rule { ref id, .. } if id == "scnlog"));
}

#[test]
fn a_repeated_id_refuses_the_pack() {
    let mut twice = pack();
    twice.rules.push(example());
    let payload = serde_json::to_value(&twice).expect("encode");
    let bytes = signed_raw(SITE_RULES_DOMAIN, &payload);
    let refused = verify_with(&bytes, &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.duplicate_id");
    assert!(sign(KEY_ID, &key(), &twice).is_err());
}

#[test]
fn signing_refuses_a_foreign_format_version() {
    let mut future = pack();
    future.format_version = 2;
    assert!(sign(KEY_ID, &key(), &future).is_err());
}

#[test]
fn garbage_is_malformed() {
    let refused = verify_with(b"not json", &trust(), None, now()).expect_err("refused");
    assert_eq!(refused.code(), "site_rules.malformed");
}
