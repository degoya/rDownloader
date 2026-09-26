//! Tests for [`super`]: each refusal in the documented order, with its stable code, and the
//! publishing half against a real signed package.

use chrono::Duration;
use rd_sign::{SigningKey, TrustStore};

use super::*;
use crate::format_package_digest;

const KEY_ID: &str = "test-repository";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const FINGERPRINT: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).expect("timestamp")
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[9; 32])
}

fn trust() -> TrustStore {
    let store = TrustStore::new();
    store
        .trust(KEY_ID.to_owned(), key().verifying_key())
        .expect("trust");
    store
}

fn package() -> IndexPackage {
    IndexPackage {
        id: "019d0000-0000-7000-8000-00000000abcd".parse().expect("id"),
        name: "Fixture".to_owned(),
        version: "1.2.3".to_owned(),
        plugin_type: PluginType::Resolver,
        api_version: "0.9.0".to_owned(),
        min_app_version: Some("1.4.0".to_owned()),
        package_digest: DIGEST.to_owned(),
        size: 4096,
        url: "fixture-1.2.3.rdplug".to_owned(),
        publisher: Publisher {
            key_id: "fixture-v1".to_owned(),
            fingerprint: FINGERPRINT.to_owned(),
            author: "Fixture Author".to_owned(),
        },
        permissions: Permissions {
            granted: vec!["net_http".to_owned()],
            http_domains: vec!["example.test".to_owned()],
            stream_hosts: Vec::new(),
        },
        release_notes: Some("Fixes the countdown.\nNo new permissions.".to_owned()),
    }
}

fn index() -> PluginIndex {
    PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 5,
        issued_at: now() - Duration::hours(1),
        not_after: now() + Duration::days(30),
        packages: vec![package()],
        revoked: Revocations::default(),
    }
}

fn signed(index: &PluginIndex) -> Vec<u8> {
    sign(KEY_ID, &key(), index).expect("sign")
}

/// Signs a payload [`sign`] would refuse, so a refusal can be tested end to end.
fn signed_raw(domain: &str, payload: &serde_json::Value) -> Vec<u8> {
    let document = rd_sign::sign_document(domain, KEY_ID, &key(), payload).expect("sign");
    serde_json::to_vec(&document).expect("encode")
}

fn refused(bytes: &[u8], known: Option<u64>) -> IndexError {
    verify_with(bytes, &trust(), known, now()).expect_err("refused")
}

/// Signs `index` after `edit` has changed its JSON, and returns the refusal.
fn refused_after(edit: impl FnOnce(&mut serde_json::Value)) -> IndexError {
    let mut payload = serde_json::to_value(index()).expect("encode");
    edit(&mut payload);
    refused(&signed_raw(PLUGIN_INDEX_DOMAIN, &payload), None)
}

#[test]
fn a_signed_index_verifies_and_yields_its_packages() {
    let verified = verify_with(&signed(&index()), &trust(), Some(4), now()).expect("verify");
    assert_eq!(verified.sequence, 5);
    assert_eq!(verified.packages.len(), 1);
    let entry = &verified.packages[0];
    assert_eq!(entry.contract(), "rdownloader:plugin@0.9.0");
    assert_eq!(
        format_package_digest(&entry.digest().expect("digest")),
        DIGEST
    );
    assert_eq!(entry.publisher.fingerprint, FINGERPRINT);
}

#[test]
fn a_tampered_byte_is_a_bad_signature() {
    let bytes = signed(&index());
    let text = String::from_utf8(bytes).expect("utf-8");
    assert!(text.contains("\"1.2.3\""));
    let tampered = text.replacen("\"1.2.3\"", "\"1.2.4\"", 1).into_bytes();
    let error = refused(&tampered, None);
    assert_eq!(error.code(), "plugin_index.bad_signature");
}

/// A signature made for another document kind under the same key does not hold here.
#[test]
fn a_signature_for_another_domain_is_refused() {
    let payload = serde_json::to_value(index()).expect("encode");
    let as_tool_manifest = signed_raw("rdownloader.tool-manifest.v1", &payload);
    assert_eq!(
        refused(&as_tool_manifest, None).code(),
        "plugin_index.bad_signature"
    );
}

#[test]
fn another_key_is_refused_whether_or_not_it_claims_the_trusted_id() {
    let impostor = SigningKey::from_bytes(&[3; 32]);
    let claims_our_id = sign(KEY_ID, &impostor, &index()).expect("sign");
    assert_eq!(
        refused(&claims_our_id, None).code(),
        "plugin_index.bad_signature"
    );
    let own_id = sign("somebody-else", &impostor, &index()).expect("sign");
    assert_eq!(refused(&own_id, None).code(), "plugin_index.untrusted");
    // The compiled-in repository root never knows a test key.
    let error = verify(&signed(&index()), None, now()).expect_err("no root");
    assert_eq!(error.code(), "plugin_index.untrusted");
}

#[test]
fn an_expired_index_is_stale() {
    let mut expired = index();
    expired.issued_at = now() - Duration::days(40);
    expired.not_after = now() - Duration::minutes(1);
    let error = refused(&signed(&expired), None);
    assert_eq!(error.code(), "plugin_index.stale");
    assert!(matches!(
        error,
        IndexError::Stale(replay::StaleError::Expired { .. })
    ));
}

/// The replay case: yesterday's genuine index, served again after a newer one was accepted.
#[test]
fn a_replayed_index_is_stale() {
    let bytes = signed(&index());
    for known in [5, 6] {
        let error = refused(&bytes, Some(known));
        assert!(matches!(
            error,
            IndexError::Stale(replay::StaleError::Replayed { saw: 5, .. })
        ));
    }
}

#[test]
fn an_oversize_index_is_refused_before_it_is_parsed() {
    let bytes = vec![b' '; MAX_INDEX_BYTES + 1];
    assert!(matches!(refused(&bytes, None), IndexError::TooLarge));
}

#[test]
fn an_unknown_schema_version_refuses_the_whole_index() {
    let error = refused_after(|payload| payload["schema_version"] = serde_json::json!(2));
    assert_eq!(error.code(), "plugin_index.schema_version_unsupported");
    assert!(matches!(error, IndexError::SchemaVersion { saw: 2 }));
}

#[test]
fn an_unknown_field_or_a_missing_expiry_is_malformed() {
    let error = refused_after(|payload| payload["mirror"] = serde_json::json!("x"));
    assert_eq!(error.code(), "plugin_index.malformed");
    let error = refused_after(|payload| payload["packages"][0]["script"] = serde_json::json!("x"));
    assert_eq!(error.code(), "plugin_index.malformed");
    let error = refused_after(|payload| {
        payload.as_object_mut().expect("object").remove("not_after");
    });
    assert_eq!(error.code(), "plugin_index.malformed");
}

#[test]
fn a_revoked_index_document_is_refused() {
    let bytes = signed(&index());
    let trust = trust();
    let document = SignedDocument::parse(&bytes).expect("parse");
    trust
        .revoke_digest(document.digest(PLUGIN_INDEX_DOMAIN))
        .expect("revoke");
    let error = verify_with(&bytes, &trust, None, now()).expect_err("refused");
    assert_eq!(error.code(), "plugin_index.revoked");
}

/// One edit that makes an otherwise valid index invalid.
type Edit = fn(&mut PluginIndex);

/// Pins a non-capturing closure to the one signature every case shares.
fn rule(edit: Edit) -> Edit {
    edit
}

/// Every content rule, each on its own otherwise valid index.
#[test]
fn invalid_content_names_itself_and_refuses_the_index() {
    let cases: Vec<(&str, Edit)> = vec![
        (
            "plain http",
            rule(|index: &mut PluginIndex| {
                index.packages[0].url = "http://example.test/a.rdplug".into()
            }),
        ),
        (
            "escaping path",
            rule(|index: &mut PluginIndex| index.packages[0].url = "../other/a.rdplug".into()),
        ),
        (
            "rooted path",
            rule(|index: &mut PluginIndex| index.packages[0].url = "/a.rdplug".into()),
        ),
        (
            "credentials",
            rule(|index: &mut PluginIndex| {
                index.packages[0].url = "https://user:pw@example.test/a.rdplug".into()
            }),
        ),
        (
            "upper-case digest",
            rule(|index: &mut PluginIndex| {
                index.packages[0].package_digest = DIGEST.to_uppercase()
            }),
        ),
        (
            "short digest",
            rule(|index: &mut PluginIndex| index.packages[0].package_digest = "abc".into()),
        ),
        (
            "zero size",
            rule(|index: &mut PluginIndex| index.packages[0].size = 0),
        ),
        (
            "huge size",
            rule(|index: &mut PluginIndex| index.packages[0].size = MAX_PACKAGE_BYTES + 1),
        ),
        (
            "not semver",
            rule(|index: &mut PluginIndex| index.packages[0].version = "latest".into()),
        ),
        (
            "bad contract",
            rule(|index: &mut PluginIndex| index.packages[0].api_version = "0.9".into()),
        ),
        (
            "long notes",
            rule(|index: &mut PluginIndex| {
                index.packages[0].release_notes = Some("x".repeat(MAX_RELEASE_NOTES_CHARS + 1))
            }),
        ),
        (
            "control in notes",
            rule(|index: &mut PluginIndex| {
                index.packages[0].release_notes = Some("a\u{1b}[31m".into())
            }),
        ),
        (
            "empty name",
            rule(|index: &mut PluginIndex| index.packages[0].name = " ".into()),
        ),
        (
            "duplicate",
            rule(|index: &mut PluginIndex| index.packages.push(package())),
        ),
        (
            "offered and withdrawn",
            rule(|index: &mut PluginIndex| index.revoked.package_digests.push(DIGEST.into())),
        ),
        (
            "withdrawn key",
            rule(|index: &mut PluginIndex| {
                index.revoked.keys.push(RevokedKey {
                    key_id: "fixture-v1".into(),
                    fingerprint: FINGERPRINT.into(),
                })
            }),
        ),
        (
            "bad withdrawn digest",
            rule(|index: &mut PluginIndex| index.revoked.package_digests.push("zz".into())),
        ),
        (
            "sequence zero",
            rule(|index: &mut PluginIndex| index.sequence = 0),
        ),
        (
            "window too long",
            rule(|index: &mut PluginIndex| {
                index.not_after = index.issued_at + Duration::days(MAX_VALIDITY_DAYS + 1)
            }),
        ),
        (
            "backwards window",
            rule(|index: &mut PluginIndex| {
                // Within the clock-skew allowance, so the window check is what refuses it.
                index.issued_at = now() + Duration::minutes(30);
                index.not_after = index.issued_at;
            }),
        ),
        (
            "too many packages",
            rule(|index: &mut PluginIndex| {
                index.packages = (0..=MAX_INDEX_PACKAGES).map(|_| package()).collect()
            }),
        ),
    ];
    for (label, edit) in cases {
        let mut broken = index();
        edit(&mut broken);
        assert!(sign(KEY_ID, &key(), &broken).is_err(), "{label}: signed");
        let payload = serde_json::to_value(&broken).expect("encode");
        let error = refused(&signed_raw(PLUGIN_INDEX_DOMAIN, &payload), None);
        assert_eq!(error.code(), "plugin_index.invalid", "{label}: {error}");
    }
}

#[test]
fn a_withdrawal_list_travels_and_parses_into_digests() {
    let mut with_withdrawal = index();
    let other = "ab".repeat(32);
    with_withdrawal.revoked.package_digests.push(other.clone());
    with_withdrawal.revoked.keys.push(RevokedKey {
        key_id: "leaked-v1".to_owned(),
        fingerprint: "cd".repeat(32),
    });
    let verified = verify_with(&signed(&with_withdrawal), &trust(), None, now()).expect("verify");
    assert_eq!(
        verified.revoked_package_digests().expect("digests"),
        vec![[0xab; 32]]
    );
    assert_eq!(verified.revoked.keys[0].key_id, "leaked-v1");
}

#[test]
fn a_relative_url_resolves_below_the_index_and_stays_https() {
    let entry = package();
    let index_url: url::Url =
        "https://github.com/o/r/releases/download/v1.4.0/rdownloader-plugin-index.json"
            .parse()
            .expect("url");
    assert_eq!(
        entry.resolve_url(&index_url).expect("resolve").as_str(),
        "https://github.com/o/r/releases/download/v1.4.0/fixture-1.2.3.rdplug"
    );
    let plain: url::Url = "http://example.test/index.json".parse().expect("url");
    assert!(entry.resolve_url(&plain).is_err(), "inherits plain http");
}

#[test]
fn a_package_url_is_a_file_name_or_joined_onto_an_https_base() {
    assert_eq!(
        package_url(None, "a-1.0.0.rdplug").expect("url"),
        "a-1.0.0.rdplug"
    );
    assert_eq!(
        package_url(Some("https://example.test/v1.4.0/"), "a-1.0.0.rdplug").expect("url"),
        "https://example.test/v1.4.0/a-1.0.0.rdplug"
    );
    assert!(package_url(Some("https://example.test/v1.4.0"), "a.rdplug").is_err());
    assert!(package_url(Some("http://example.test/"), "a.rdplug").is_err());
    assert!(package_url(None, "../a.rdplug").is_err());
}

/// The publishing half against a package the real packager signed.
#[test]
fn a_signed_package_is_described_by_its_package_digest() {
    let signing = SigningKey::from_bytes(&[11; 32]);
    let manifest = crate::tests::fixture_manifest(&crate::public_key_base64(&signing));
    let archive = crate::package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], Some(&signing))
        .expect("package");
    let entry = describe_package(
        &archive,
        "fixture-1.2.3.rdplug".to_owned(),
        Some("  Line one\r\nLine two  ".to_owned()),
    )
    .expect("describe");
    let digest = crate::package_digest(manifest.as_bytes(), EMPTY_COMPONENT, &[]);
    assert_eq!(entry.package_digest, format_package_digest(&digest));
    assert_eq!(entry.size, archive.len() as u64);
    assert_eq!(entry.publisher.key_id, "fixture-v1");
    assert_eq!(
        entry.publisher.fingerprint,
        rd_sign::key_fingerprint(&signing.verifying_key())
    );
    assert_eq!(entry.publisher.author, "Fixture Author");
    assert_eq!(entry.permissions.granted, vec!["net_http".to_owned()]);
    assert_eq!(
        entry.permissions.http_domains,
        vec!["example.test".to_owned()]
    );
    assert_eq!(entry.release_notes.as_deref(), Some("Line one\nLine two"));

    let mut published = index();
    published.packages = vec![entry];
    verify_with(&signed(&published), &trust(), None, now()).expect("verify");
}

#[test]
fn an_unsigned_package_cannot_be_listed() {
    let manifest = crate::tests::fixture_manifest(crate::tests::TEST_PUBLIC_KEY);
    let archive =
        crate::package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], None).expect("package");
    let error = describe_package(&archive, "a.rdplug".to_owned(), None).expect_err("refused");
    assert!(error.to_string().contains("unsigned"), "{error}");
}

/// The withdrawal list the release publishes has to be one every reader accepts.
#[test]
fn the_committed_withdrawal_list_is_valid() {
    let revocations: Revocations =
        serde_json::from_slice(include_bytes!("../resources/plugin-index-revocations.json"))
            .expect("parse");
    revocations.validate().expect("valid");
}

/// Every bundled plugin's permissions fit an index entry: the 1.4.0 release failed on
/// `xfs-generic`, whose 422 HTTP domains exceeded the list bound.
#[test]
fn every_bundled_plugin_fits_an_index_entry() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let plugins = std::path::Path::new(&manifest_dir).join("../../plugins");
    let mut checked = 0;
    for entry in std::fs::read_dir(&plugins).expect("plugins directory") {
        let path = entry.expect("entry").path().join("manifest.toml");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let manifest: crate::PluginManifest =
            toml::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut index = index();
        index.packages[0].permissions = Permissions::of(&manifest);
        index
            .validate()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        checked += 1;
    }
    assert!(
        checked > 50,
        "only {checked} manifests found under {}",
        plugins.display()
    );
}
