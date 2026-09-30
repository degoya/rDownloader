//! What a manipulated, replayed or misplaced manifest does: it is refused, with the code that
//! says why (RD-180-01).

use chrono::{DateTime, Duration, Utc};
use rd_sign::{SigningKey, StaleError, TrustStore};

use super::*;

const KEY_ID: &str = "rdownloader-update-v1";

pub(crate) fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

pub(crate) fn trust_for(key: &SigningKey) -> TrustStore {
    let trust = TrustStore::new();
    trust
        .trust(KEY_ID.to_owned(), key.verifying_key())
        .expect("trust");
    trust
}

pub(crate) fn at(days: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).expect("timestamp") + Duration::days(days)
}

pub(crate) fn artifact(platform: &str, arch: &str, kind: &str) -> Artifact {
    Artifact {
        platform: platform.to_owned(),
        arch: arch.to_owned(),
        kind: kind.to_owned(),
        url: format!(
            "https://github.com/degoya/rDownloader/releases/download/v1.8.0/rdownloader-{platform}-{arch}.{kind}"
        ),
        sha256: "ab".repeat(32),
        size: 1024,
    }
}

pub(crate) fn manifest(channel: Channel, version: &str, sequence: u64) -> UpdateManifest {
    UpdateManifest {
        schema_version: UPDATE_MANIFEST_SCHEMA_VERSION,
        sequence,
        issued_at: at(0),
        not_after: at(180),
        channel,
        version: version.to_owned(),
        released_at: at(0),
        notes: "Added\n- Update check".to_owned(),
        artifacts: vec![
            artifact("linux", "x86_64", kind::ARCHIVE),
            artifact("windows", "x86_64", kind::MSI),
        ],
        schema_change: None,
    }
}

pub(crate) fn signed(manifest: &UpdateManifest) -> Vec<u8> {
    sign(KEY_ID, &key(), manifest).expect("sign")
}

#[test]
fn a_signed_manifest_verifies_and_round_trips() {
    let original = manifest(Channel::Stable, "1.8.0", 5);
    let verified = verify_with(
        &signed(&original),
        &trust_for(&key()),
        Channel::Stable,
        None,
        at(1),
    )
    .expect("verify");
    assert_eq!(verified.version, "1.8.0");
    assert_eq!(verified.sequence, 5);
    assert_eq!(verified.artifacts, original.artifacts);
}

#[test]
fn a_manifest_signed_by_another_key_is_untrusted() {
    let bytes = sign(
        KEY_ID,
        &SigningKey::from_bytes(&[9; 32]),
        &manifest(Channel::Stable, "1.8.0", 1),
    )
    .expect("sign");
    // The other key under the same id: a signature that names the trusted key and does not
    // hold is tampering, not a trust decision.
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("wrong key");
    assert_eq!(error.code(), "update.bad_signature", "{error}");
    // A key the store does not know at all.
    let error =
        verify_with(&bytes, &TrustStore::new(), Channel::Stable, None, at(1)).expect_err("no key");
    assert_eq!(error.code(), "update.untrusted", "{error}");
}

#[test]
fn a_tampered_manifest_is_refused() {
    let bytes = signed(&manifest(Channel::Stable, "1.8.0", 1));
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["payload"]["artifacts"][0]["url"] =
        serde_json::Value::String("https://evil.example/rdownloader.tar.gz".to_owned());
    let tampered = serde_json::to_vec(&document).expect("encode");
    let error = verify_with(&tampered, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("tampered");
    assert_eq!(error.code(), "update.bad_signature");
}

/// The domain separator: a signature by the right key over another document kind is not an
/// update manifest.
#[test]
fn a_signature_for_another_domain_does_not_verify() {
    let payload = manifest(Channel::Stable, "1.8.0", 1);
    let document = rd_sign::sign_document("rdownloader.plugin-index.v1", KEY_ID, &key(), &payload)
        .expect("sign");
    let bytes = serde_json::to_vec(&document).expect("encode");
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("wrong domain");
    assert_eq!(error.code(), "update.bad_signature");
}

#[test]
fn a_manifest_below_the_floor_is_a_replay_and_one_at_it_is_the_same_manifest() {
    let bytes = signed(&manifest(Channel::Stable, "1.8.0", 5));
    let trust = trust_for(&key());
    let error = verify_with(&bytes, &trust, Channel::Stable, Some(6), at(1)).expect_err("replay");
    assert!(
        matches!(
            error,
            UpdateError::Stale(StaleError::Replayed { saw: 5, known: 6 })
        ),
        "{error}"
    );
    assert_eq!(error.code(), "update.stale");
    verify_with(&bytes, &trust, Channel::Stable, Some(5), at(1)).expect("the accepted manifest");
}

#[test]
fn an_expired_manifest_is_refused() {
    let bytes = signed(&manifest(Channel::Stable, "1.8.0", 1));
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(181))
        .expect_err("expired");
    assert!(
        matches!(error, UpdateError::Stale(StaleError::Expired { .. })),
        "{error}"
    );
}

#[test]
fn a_manifest_from_the_future_is_refused() {
    let bytes = signed(&manifest(Channel::Stable, "1.8.0", 1));
    let error =
        verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(-7)).expect_err("future");
    assert!(
        matches!(error, UpdateError::Stale(StaleError::FromTheFuture { .. })),
        "{error}"
    );
}

/// A genuine beta manifest served at the stable address must not reach a stable installation.
#[test]
fn a_manifest_for_the_other_channel_is_refused() {
    let bytes = signed(&manifest(Channel::Beta, "1.8.0-beta.2", 1));
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("wrong channel");
    assert_eq!(error.code(), "update.wrong_channel");
}

#[test]
fn a_version_that_contradicts_its_channel_is_refused_at_signing_and_at_verification() {
    assert!(
        sign(
            KEY_ID,
            &key(),
            &manifest(Channel::Stable, "1.8.0-beta.1", 1)
        )
        .is_err()
    );
    assert!(sign(KEY_ID, &key(), &manifest(Channel::Beta, "1.8.0", 1)).is_err());
    // Signed around `sign`'s check, as a careless publisher could.
    let document = rd_sign::sign_document(
        UPDATE_MANIFEST_DOMAIN,
        KEY_ID,
        &key(),
        &manifest(Channel::Stable, "1.8.0-beta.1", 1),
    )
    .expect("sign");
    let bytes = serde_json::to_vec(&document).expect("encode");
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("beta on stable");
    assert_eq!(error.code(), "update.invalid");
}

#[test]
fn a_future_schema_version_is_refused_after_the_signature() {
    let mut payload = serde_json::to_value(manifest(Channel::Stable, "1.8.0", 1)).expect("value");
    payload["schema_version"] = serde_json::json!(2);
    let document =
        rd_sign::sign_document(UPDATE_MANIFEST_DOMAIN, KEY_ID, &key(), &payload).expect("sign");
    let bytes = serde_json::to_vec(&document).expect("encode");
    let error =
        verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1)).expect_err("schema");
    assert_eq!(error.code(), "update.schema_version_unsupported");
    // Unsigned, the same document is an untrusted one, not an unknown version.
    let error = verify_with(&bytes, &TrustStore::new(), Channel::Stable, None, at(1))
        .expect_err("unsigned");
    assert_eq!(error.code(), "update.untrusted");
}

/// A later release may add fields; an older reader ignores them rather than refusing the update
/// check. What it must not misread is a new schema version, refused above.
#[test]
fn fields_a_later_release_adds_are_ignored() {
    let mut payload = serde_json::to_value(manifest(Channel::Stable, "1.8.0", 1)).expect("value");
    payload["minimum_backup_version"] = serde_json::json!("1.7.0");
    payload["artifacts"][0]["signature_url"] = serde_json::json!("https://example.test/a.sig");
    let document =
        rd_sign::sign_document(UPDATE_MANIFEST_DOMAIN, KEY_ID, &key(), &payload).expect("sign");
    let bytes = serde_json::to_vec(&document).expect("encode");
    let verified = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect("an additive field verifies");
    assert_eq!(verified.version, "1.8.0");
    assert_eq!(verified.artifacts.len(), 2);
}

#[test]
fn the_schema_change_is_signed_and_absent_means_changed() {
    let absent = manifest(Channel::Stable, "1.8.0", 1);
    assert!(absent.changes_schema());
    // Not written when unknown, so a manifest without it stays byte-for-byte what it was.
    assert!(
        serde_json::to_value(&absent)
            .expect("value")
            .get("schema_change")
            .is_none()
    );
    for said in [false, true] {
        let mut original = manifest(Channel::Stable, "1.8.0", 1);
        original.schema_change = Some(said);
        let verified = verify_with(
            &signed(&original),
            &trust_for(&key()),
            Channel::Stable,
            None,
            at(1),
        )
        .expect("verify");
        assert_eq!(verified.schema_change, Some(said));
        assert_eq!(verified.changes_schema(), said);
    }
}

#[test]
fn artifacts_are_held_to_https_a_real_digest_and_one_entry_each() {
    let mut plain_http = manifest(Channel::Stable, "1.8.0", 1);
    plain_http.artifacts[0].url = "http://github.com/a.tar.gz".to_owned();
    assert!(plain_http.validate().is_err());

    let mut short_hash = manifest(Channel::Stable, "1.8.0", 1);
    short_hash.artifacts[0].sha256 = "abc".to_owned();
    assert!(short_hash.validate().is_err());

    let mut empty = manifest(Channel::Stable, "1.8.0", 1);
    empty.artifacts[0].size = 0;
    assert!(empty.validate().is_err());

    let mut twice = manifest(Channel::Stable, "1.8.0", 1);
    twice.artifacts.push(twice.artifacts[0].clone());
    assert!(twice.validate().is_err());

    // A platform this build does not know is fine; it is simply never selected.
    let mut later = manifest(Channel::Stable, "1.8.0", 1);
    later.artifacts.push(artifact("freebsd", "riscv64", "pkg"));
    later
        .validate()
        .expect("an unknown platform is listed, not refused");
}

#[test]
fn an_oversized_document_is_refused_before_it_is_parsed() {
    let bytes = vec![b' '; MAX_MANIFEST_BYTES + 1];
    let error = verify_with(&bytes, &trust_for(&key()), Channel::Stable, None, at(1))
        .expect_err("too large");
    assert_eq!(error.code(), "update.too_large");
}

/// The compiled-in root is the owner's update key: a manifest signed by any other key under
/// the same id does not verify against it.
#[test]
fn the_compiled_in_root_refuses_a_foreign_key() {
    let error = verify(
        &signed(&manifest(Channel::Stable, "1.8.0", 1)),
        Channel::Stable,
        None,
        at(1),
    )
    .expect_err("a test key is not the release root");
    assert_eq!(error.code(), "update.bad_signature", "{error}");
}
