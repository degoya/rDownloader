//! Object storage profiles and the multipart upload records that outlive a restart
//! (RD-150-04, RD-150-05).

use chrono::Utc;
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::{Database, NewObjectStorageProfile, ObjectUpload, ObjectUploadPart};

async fn open() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("objects.sqlite3"))
        .await
        .expect("database");
    (directory, database)
}

fn input(name: &str) -> NewObjectStorageProfile {
    NewObjectStorageProfile {
        name: name.to_owned(),
        provider: ObjectStorageProvider::S3,
        endpoint: Some("https://minio.example:9000".to_owned()),
        region: Some("us-east-1".to_owned()),
        bucket: None,
        addressing: ObjectAddressing::Path,
        credential_source: ObjectCredentialSource::Static,
        access_key_id: Some("AKIDEXAMPLE".to_owned()),
        account: None,
        secret_ref: Some("vault://secret-one".to_owned()),
        session_token_ref: Some("vault://token-one".to_owned()),
        checksums: true,
        enabled: true,
        ambient_custom_endpoint: false,
    }
}

fn upload(profile_id: rd_core::ObjectStorageProfileId, id: &str) -> ObjectUpload {
    ObjectUpload {
        id: id.to_owned(),
        profile_id,
        owner: "package-1".to_owned(),
        bucket: "media-bucket".to_owned(),
        object_key: "releases/a.bin".to_owned(),
        local_path: "/downloads/a.bin".to_owned(),
        local_size: 40 * 1024 * 1024,
        local_modified: Some("2026-09-27T10:00:00Z".to_owned()),
        part_size: 16 * 1024 * 1024,
        upload_id: Some(format!("mpu-{id}")),
        checksums: true,
        completed_at: None,
        created_at: Utc::now(),
        parts: Vec::new(),
    }
}

#[tokio::test]
async fn a_profile_keeps_its_references_out_of_every_answer() {
    let (_directory, database) = open().await;
    let created = database
        .create_object_storage_profile(input("MinIO"))
        .await
        .expect("create");
    assert!(created.has_secret && created.has_session_token);
    let json = serde_json::to_string(&created).expect("json");
    assert!(!json.contains("vault://"), "{json}");

    let duplicate = database
        .create_object_storage_profile(input("MinIO"))
        .await
        .expect_err("same name");
    assert_eq!(
        rd_db::store_kind(&duplicate),
        Some(rd_db::StoreErrorKind::Duplicate)
    );

    // Dropping the session token hands its reference back for cleanup, and only that one.
    let mut changed = input("MinIO");
    changed.session_token_ref = None;
    let (updated, orphaned) = database
        .update_object_storage_profile(created.id, changed)
        .await
        .expect("update");
    assert!(!updated.has_session_token);
    assert_eq!(orphaned, vec!["vault://token-one".to_owned()]);

    let orphaned = database
        .delete_object_storage_profile(created.id)
        .await
        .expect("delete");
    assert_eq!(orphaned, vec!["vault://secret-one".to_owned()]);
    assert!(
        database
            .list_object_storage_profiles()
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn an_azure_profile_keeps_its_account_and_its_signature_source() {
    let (_directory, database) = open().await;
    let created = database
        .create_object_storage_profile(NewObjectStorageProfile {
            provider: ObjectStorageProvider::Azure,
            endpoint: None,
            region: None,
            credential_source: ObjectCredentialSource::SharedAccessSignature,
            access_key_id: None,
            account: Some("mediaarchive".to_owned()),
            session_token_ref: None,
            ..input("Blob")
        })
        .await
        .expect("create");
    let stored = database
        .object_storage_profile(created.id)
        .await
        .expect("read")
        .expect("profile");
    assert_eq!(stored.provider, ObjectStorageProvider::Azure);
    assert_eq!(
        stored.credential_source,
        ObjectCredentialSource::SharedAccessSignature
    );
    assert_eq!(stored.account.as_deref(), Some("mediaarchive"));
    assert!(stored.has_secret);
}

/// RD-1190-20: the yes to ambient credentials at a custom endpoint is stored with the profile
/// and taken back by an update that leaves it out.
#[tokio::test]
async fn the_ambient_endpoint_opt_in_is_kept_until_it_is_withdrawn() {
    let (_directory, database) = open().await;
    let ambient = NewObjectStorageProfile {
        credential_source: ObjectCredentialSource::Ambient,
        access_key_id: None,
        secret_ref: None,
        session_token_ref: None,
        ambient_custom_endpoint: true,
        ..input("Machine")
    };
    let created = database
        .create_object_storage_profile(ambient.clone())
        .await
        .expect("create");
    assert!(created.ambient_custom_endpoint);
    assert!(!created.ambient_endpoint_unconfirmed());
    let (updated, _) = database
        .update_object_storage_profile(
            created.id,
            NewObjectStorageProfile {
                ambient_custom_endpoint: false,
                ..ambient
            },
        )
        .await
        .expect("update");
    assert!(updated.ambient_endpoint_unconfirmed());
}

#[tokio::test]
async fn confirmed_parts_are_there_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("objects.sqlite3");
    let profile_id = {
        let database = Database::open(&path).await.expect("database");
        let profile = database
            .create_object_storage_profile(input("MinIO"))
            .await
            .expect("create");
        database
            .begin_object_upload(upload(profile.id, "u1"))
            .await
            .expect("begin");
        for part_number in [1_u32, 0] {
            database
                .record_object_upload_part(
                    "u1".to_owned(),
                    ObjectUploadPart {
                        part_number,
                        content_id: format!("\"etag-{part_number}\""),
                        size: 16 * 1024 * 1024,
                    },
                )
                .await
                .expect("part");
        }
        profile.id
    };
    // A fresh process: the parts come back in order, with what the completion needs.
    let database = Database::open(&path).await.expect("reopen");
    let recorded = database
        .object_upload(profile_id, "media-bucket", "releases/a.bin")
        .await
        .expect("read")
        .expect("recorded");
    assert_eq!(recorded.upload_id.as_deref(), Some("mpu-u1"));
    let numbers: Vec<u32> = recorded.parts.iter().map(|part| part.part_number).collect();
    assert_eq!(numbers, vec![0, 1]);
    assert_eq!(recorded.parts[0].content_id, "\"etag-0\"");

    // Starting over for the same object replaces the record and its parts.
    database
        .begin_object_upload(upload(profile_id, "u2"))
        .await
        .expect("restart");
    let replaced = database
        .object_upload(profile_id, "media-bucket", "releases/a.bin")
        .await
        .expect("read")
        .expect("recorded");
    assert_eq!(replaced.id, "u2");
    assert!(replaced.parts.is_empty());

    database
        .complete_object_upload("u2".to_owned())
        .await
        .expect("complete");
    let completed = database
        .object_upload(profile_id, "media-bucket", "releases/a.bin")
        .await
        .expect("read")
        .expect("recorded");
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.upload_id, None);
}

#[tokio::test]
async fn forgetting_needs_an_id_or_an_owner() {
    let (_directory, database) = open().await;
    let profile = database
        .create_object_storage_profile(input("MinIO"))
        .await
        .expect("create");
    database
        .begin_object_upload(upload(profile.id, "u1"))
        .await
        .expect("begin");
    // Naming neither must not empty the table.
    assert_eq!(
        database
            .forget_object_uploads(None, None)
            .await
            .expect("forget"),
        0
    );
    assert_eq!(
        database
            .object_uploads(Some(profile.id), None)
            .await
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        database
            .forget_object_uploads(None, Some("package-1".to_owned()))
            .await
            .expect("forget"),
        1
    );
    // A profile's deletion takes its upload records with it.
    database
        .begin_object_upload(upload(profile.id, "u3"))
        .await
        .expect("begin");
    database
        .delete_object_storage_profile(profile.id)
        .await
        .expect("delete");
    assert!(
        database
            .object_uploads(None, None)
            .await
            .expect("list")
            .is_empty()
    );
}
