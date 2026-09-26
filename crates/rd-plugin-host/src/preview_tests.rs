//! Tests for [`super`]: the preview reports without deciding, and the verifier refuses a
//! withdrawn key before it would ask for trust.

use rd_sign::SigningKey;

use super::*;
use crate::{VerifyError, package_plugin, public_key_base64, tests::fixture_manifest};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[21; 32])
}

/// Writes an archive the packager would refuse, signed by `key` over its own members.
fn raw_archive(manifest: &str, key: &SigningKey) -> Vec<u8> {
    use std::io::Write;
    let digest = crate::package_digest(manifest.as_bytes(), EMPTY_COMPONENT, &[]);
    let signature = rd_sign::sign_detached(key, &digest);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in [
        ("manifest.toml", manifest.as_bytes()),
        ("component.wasm", EMPTY_COMPONENT),
        ("signature.ed25519", signature.as_bytes()),
    ] {
        writer.start_file(name, options).expect("member");
        writer.write_all(bytes).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn signed_package() -> Vec<u8> {
    let manifest = fixture_manifest(&public_key_base64(&signing()));
    package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], Some(&signing())).expect("package")
}

#[test]
fn a_package_from_an_unknown_key_is_described_with_its_permissions() {
    let verifier = PluginVerifier::new(false);
    let preview = preview_package(&signed_package(), &verifier).expect("preview");
    assert_eq!(preview.name, "Fixture");
    assert_eq!(preview.version, "1.2.3");
    assert_eq!(preview.key, KeyStatus::Untrusted);
    let publisher = preview.publisher.as_ref().expect("publisher");
    assert_eq!(publisher.key_id, "fixture-v1");
    assert_eq!(
        publisher.fingerprint,
        key_fingerprint(&signing().verifying_key())
    );
    assert_eq!(publisher.author, "Fixture Author");
    assert_eq!(preview.permissions.granted, vec!["net_http".to_owned()]);
    assert_eq!(
        preview.permissions.http_domains,
        vec!["example.test".to_owned()]
    );
    assert!(!preview.withdrawn);
    assert_eq!(preview.incompatible, None);
    assert!(preview.installable());
    // A preview decides nothing: the key is still unknown afterwards.
    assert!(!verifier.is_trusted("fixture-v1").expect("trusted"));
}

#[test]
fn a_trusted_key_is_previewed_too() {
    let verifier = PluginVerifier::new(false);
    verifier
        .trust_key("fixture-v1".to_owned(), signing().verifying_key())
        .expect("trust");
    let preview = preview_package(&signed_package(), &verifier).expect("preview");
    assert_eq!(preview.key, KeyStatus::Trusted);
    assert!(!preview.permissions.granted.is_empty());
}

#[test]
fn another_key_under_a_trusted_id_is_a_mismatch() {
    let verifier = PluginVerifier::new(false);
    verifier
        .trust_key(
            "fixture-v1".to_owned(),
            SigningKey::from_bytes(&[4; 32]).verifying_key(),
        )
        .expect("trust");
    let preview = preview_package(&signed_package(), &verifier).expect("preview");
    assert_eq!(preview.key, KeyStatus::Mismatch);
    assert!(!preview.installable());
}

#[test]
fn a_withdrawn_digest_and_a_withdrawn_key_are_reported() {
    let bytes = signed_package();
    let verifier = PluginVerifier::new(false);
    verifier
        .revoke_package_digest(archive_digest(&bytes).expect("digest"))
        .expect("revoke");
    let preview = preview_package(&bytes, &verifier).expect("preview");
    assert!(preview.withdrawn);
    assert!(!preview.installable());

    let verifier = PluginVerifier::new(false);
    verifier
        .withdraw_key(&key_fingerprint(&signing().verifying_key()))
        .expect("withdraw");
    let preview = preview_package(&bytes, &verifier).expect("preview");
    assert_eq!(preview.key, KeyStatus::Withdrawn);
}

/// A withdrawn key is refused outright, not offered for trust on first use.
#[test]
fn the_verifier_refuses_a_withdrawn_key_before_asking_for_trust() {
    let bytes = signed_package();
    let verifier = PluginVerifier::new(false);
    assert!(matches!(
        verifier.verify_bytes(&bytes),
        Err(VerifyError::UntrustedKey { .. })
    ));
    verifier
        .withdraw_key(&key_fingerprint(&signing().verifying_key()).to_uppercase())
        .expect("withdraw");
    let Err(VerifyError::Other(error)) = verifier.verify_bytes(&bytes) else {
        panic!("a withdrawn key was not refused");
    };
    assert!(error.to_string().contains("withdrawn"), "{error}");
}

#[test]
fn a_tampered_package_is_not_previewed() {
    let manifest = fixture_manifest(&public_key_base64(&signing()));
    // Signed by a key other than the one the manifest names.
    let bytes = raw_archive(&manifest, &SigningKey::from_bytes(&[5; 32]));
    let error = preview_package(&bytes, &PluginVerifier::new(false)).expect_err("refused");
    assert!(error.to_string().contains("not signed"), "{error}");
}

#[test]
fn an_unsupported_contract_is_described_and_marked() {
    let manifest = fixture_manifest(&public_key_base64(&signing()))
        .replace(r#"api_version = "0.9.0""#, r#"api_version = "0.5.0""#);
    let bytes = raw_archive(&manifest, &signing());
    let preview = preview_package(&bytes, &PluginVerifier::new(false)).expect("preview");
    assert_eq!(preview.api_version, "0.5.0");
    assert!(preview.incompatible.is_some());
    assert!(!preview.installable());
}
