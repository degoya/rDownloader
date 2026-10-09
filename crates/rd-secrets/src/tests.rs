//! The vault's own tests: references, envelopes, the master key file.

use super::{
    KeyringInteractionRefused, SecretStore, master_key_from, references_in, write_private,
};

#[test]
fn references_are_found_in_columns_and_documents() {
    let column = "vault://0199a000-0000-7000-8000-000000000001";
    let document =
        r#"{"secret_ref":"vault://0199a000-0000-7000-8000-000000000002","x":"vault://not-a-uuid"}"#;
    assert_eq!(references_in(column), vec![column.to_owned()]);
    assert_eq!(
        references_in(document),
        vec!["vault://0199a000-0000-7000-8000-000000000002".to_owned()]
    );
    assert!(references_in("no reference here").is_empty());
}

/// RA-DB-07: only the spelling `new_reference` produces is a reference. Every other form
/// of the same id would be stored under it and then missed by `references_in`.
#[tokio::test]
async fn a_reference_in_another_spelling_of_its_id_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let canonical = SecretStore::new_reference();
    let id = canonical.trim_start_matches("vault://");
    for other in [
        format!("vault://{}", id.to_uppercase()),
        format!("vault://{}", id.replace('-', "")),
        format!("vault://{{{id}}}"),
        format!("vault://urn:uuid:{id}"),
    ] {
        assert!(
            store
                .put_at(&other, secrecy::SecretString::from("value".to_owned()))
                .await
                .is_err(),
            "{other} is refused"
        );
        assert!(store.get(&other).await.is_err(), "{other} reads nothing");
    }
    assert!(store.stored_references().await.expect("list").is_empty());
    store
        .put_at(&canonical, secrecy::SecretString::from("value".to_owned()))
        .await
        .expect("the canonical form is stored");
    assert_eq!(references_in(&canonical), vec![canonical.clone()]);
}

/// The listing names what `put` wrote and a stopped write's temporary, never the master key.
#[tokio::test]
async fn stored_references_list_values_and_temporaries() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let written = store.put_string("value".to_owned()).await.expect("put");
    let stopped = SecretStore::new_reference();
    let id = stopped.trim_start_matches("vault://");
    std::fs::write(directory.path().join(format!(".{id}.tmp")), b"x").expect("temporary");
    let mut expected = vec![written, stopped];
    expected.sort();
    assert_eq!(store.stored_references().await.expect("list"), expected);
}
use secrecy::ExposeSecret;

/// `open` keeps its master key beside the vault and nowhere else. Where an OS keyring is
/// reachable the old `open` put the key there instead and wrote no file — and it did so for
/// every test's vault, in the one entry the real installation uses.
#[tokio::test]
async fn open_keeps_the_master_key_in_the_file_beside_the_vault() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let key = std::fs::read(directory.path().join("master.key")).expect("master key file");
    assert_eq!(key.as_slice(), store.key.as_slice());
}

#[tokio::test]
async fn encrypted_secret_round_trips_and_is_not_plaintext() {
    let directory = tempfile::tempdir().expect("tempdir");
    write_private(&directory.path().join("master.key"), &[7_u8; 32])
        .await
        .expect("master key");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let reference = store
        .put("very-secret-value".to_owned().into())
        .await
        .expect("put");
    let id = reference.trim_start_matches("vault://");
    let envelope =
        std::fs::read_to_string(directory.path().join(format!("{id}.secret"))).expect("envelope");
    assert!(!envelope.contains("very-secret-value"));
    assert_eq!(
        store.get(&reference).await.expect("get").expose_secret(),
        "very-secret-value"
    );
}

/// A reference recorded before its value exists (RD-190-04): the value lands under exactly
/// that reference, a repeated write replaces it, and a temporary a stopped write left
/// behind neither blocks the repeat nor outlives the removal.
#[tokio::test]
async fn a_reserved_reference_is_written_repeated_and_removed_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let reference = SecretStore::new_reference();
    assert!(reference.starts_with("vault://"));
    store
        .get(&reference)
        .await
        .expect_err("nothing is stored under a fresh reference");
    let id = reference.trim_start_matches("vault://");
    let temporary = directory.path().join(format!(".{id}.tmp"));
    std::fs::write(&temporary, b"left by a stopped write").expect("stale temporary");
    store
        .put_at(&reference, "first".to_owned().into())
        .await
        .expect("put");
    store
        .put_at(&reference, "second".to_owned().into())
        .await
        .expect("repeat");
    assert_eq!(
        store.get(&reference).await.expect("get").expose_secret(),
        "second"
    );
    std::fs::write(&temporary, b"left by a stopped write").expect("stale temporary");
    store.remove(&reference).await.expect("remove");
    assert!(!temporary.exists());
    assert!(!directory.path().join(format!("{id}.secret")).exists());
    store
        .remove(&reference)
        .await
        .expect("a second removal finds nothing");
    store
        .put_at("vault://not-a-uuid", "value".to_owned().into())
        .await
        .expect_err("a reference the vault did not mint");
}

/// Key material is bytes, and a byte that is not UTF-8 must survive the round trip
/// (RD-110-33). Storing it as text without an encoding is how a key quietly becomes a
/// different key.
#[tokio::test]
async fn key_material_survives_the_round_trip_byte_for_byte() {
    let directory = tempfile::tempdir().expect("tempdir");
    write_private(&directory.path().join("master.key"), &[7_u8; 32])
        .await
        .expect("master key");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let key = [
        0x0c, 0x4c, 0x44, 0xe1, 0x28, 0xea, 0xee, 0x7a, 0x40, 0xbc, 0xbd, 0x4f, 0xfe, 0xc1, 0x96,
        0x17,
    ];
    let reference = store.put_bytes(&key).await.expect("put");
    assert!(reference.starts_with("vault://"));
    assert_eq!(store.get_bytes(&reference).await.expect("get"), key);
    // Nothing of the key is on disk in the clear.
    let id = reference.trim_start_matches("vault://");
    let envelope =
        std::fs::read_to_string(directory.path().join(format!("{id}.secret"))).expect("envelope");
    let encoded = base64::Engine::encode(&super::STANDARD, key);
    assert!(!envelope.contains(&encoded), "{envelope}");
    store.put_bytes(&[]).await.expect_err("empty key material");
}

#[cfg(unix)]
#[tokio::test]
async fn fallback_master_key_file_is_not_group_or_world_readable() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("master.key");
    write_private(&path, &[7_u8; 32]).await.expect("master key");
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    // A second write must not silently replace a master key that is already in use.
    write_private(&path, &[9_u8; 32])
        .await
        .expect_err("existing master key overwritten");
}

/// Audit S14 (RD-1110-07): an entry written before version 2 still opens, every write is a
/// version 2, and a version 2 entry renamed to another reference no longer decrypts there.
#[tokio::test]
async fn version_two_binds_an_entry_to_its_reference_and_version_one_still_reads() {
    use chacha20poly1305::{
        KeyInit, XChaCha20Poly1305, XNonce,
        aead::{Aead, Payload},
    };

    let directory = tempfile::tempdir().expect("tempdir");
    write_private(&directory.path().join("master.key"), &[7_u8; 32])
        .await
        .expect("master key");
    let store = SecretStore::open(directory.path().to_owned())
        .await
        .expect("store");
    let file = |reference: &str| {
        directory.path().join(format!(
            "{}.secret",
            reference.trim_start_matches("vault://")
        ))
    };

    // A version 1 entry, sealed the way every release before 1.11 sealed one.
    let legacy = SecretStore::new_reference();
    let nonce = [3_u8; 24];
    let ciphertext = XChaCha20Poly1305::new_from_slice(&[7_u8; 32])
        .expect("cipher")
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: b"written by 1.10",
                aad: super::AAD_V1,
            },
        )
        .expect("encrypt");
    let envelope = super::Envelope {
        version: 1,
        nonce: base64::Engine::encode(&super::STANDARD, nonce),
        ciphertext: base64::Engine::encode(&super::STANDARD, ciphertext),
    };
    std::fs::write(
        file(&legacy),
        serde_json::to_vec(&envelope).expect("envelope"),
    )
    .expect("legacy entry");
    assert_eq!(
        store.get(&legacy).await.expect("v1 reads").expose_secret(),
        "written by 1.10"
    );

    // Every write is version 2, a rewrite of the legacy reference included.
    let first = store.put_string("first".to_owned()).await.expect("put");
    let second = store.put_string("second".to_owned()).await.expect("put");
    store
        .put_at(&legacy, "rewritten".to_owned().into())
        .await
        .expect("rewrite");
    for reference in [&first, &second, &legacy] {
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file(reference)).expect("read")).expect("json");
        assert_eq!(written["version"], 2, "{reference}");
    }
    assert_eq!(
        store.get(&legacy).await.expect("get").expose_secret(),
        "rewritten"
    );

    // Swap the two files: each now sits under a reference it was not sealed for.
    let swap = directory.path().join("swap");
    std::fs::rename(file(&first), &swap).expect("move");
    std::fs::rename(file(&second), file(&first)).expect("move");
    std::fs::rename(&swap, file(&second)).expect("move");
    store
        .get(&first)
        .await
        .expect_err("a v2 entry under another reference fails its associated data");
    store
        .get(&second)
        .await
        .expect_err("a v2 entry under another reference fails its associated data");
}

/// A version this build does not know is refused, not guessed at.
#[test]
fn an_unknown_envelope_version_is_refused() {
    let id = uuid::Uuid::now_v7();
    assert!(super::associated_data(1, id).is_ok());
    assert!(super::associated_data(2, id).is_ok());
    assert!(super::associated_data(3, id).is_err());
    assert_ne!(
        super::associated_data(2, id).expect("v2"),
        super::associated_data(2, uuid::Uuid::now_v7()).expect("v2"),
        "the associated data differs per reference"
    );
}

/// RD-1200-02: a keyring that refuses to hand out the master key without asking ends the open
/// with its own error, and no key is minted -- neither a fallback file nor a keyring entry
/// replaces the one the vault was written under.
#[tokio::test]
async fn a_refused_keyring_read_mints_no_new_master_key() {
    fn refused() -> anyhow::Result<Option<[u8; 32]>> {
        Err(anyhow::Error::new(KeyringInteractionRefused))
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let error = master_key_from(directory.path(), Some(refused))
        .await
        .expect_err("the open fails");
    assert!(error.downcast_ref::<KeyringInteractionRefused>().is_some());
    assert!(!directory.path().join("master.key").exists());
}
