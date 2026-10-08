//! Object storage profiles keep their credentials to the host they were typed for
//! (RD-1190-20, `docs/security/object-storage.md` open findings 2, 3, 6 and 7): a changed
//! endpoint asks for the secret again, the machine's own credentials reach a custom endpoint
//! only on the profile's explicit yes, an upload target names an existing profile and only its
//! bound bucket, and every profile change leaves an audit record.

use axum::http::StatusCode;
use rd_core::AuditAction;
use serde_json::{Value, json};

use crate::common;

const PROFILES: &str = "/api/v1/object-storage/profiles";
const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

fn static_profile(endpoint: &str, secret: Option<&str>) -> Value {
    json!({
        "name": "Archive",
        "provider": "s3",
        "endpoint": endpoint,
        "region": "us-east-1",
        "bucket": "media-bucket",
        "credential_source": "static",
        "access_key_id": "AKIDEXAMPLE",
        "secret_access_key": secret,
    })
}

async fn created(router: &axum::Router, body: Value) -> Value {
    let (status, profile) = common::post_json(router, PROFILES, body).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    profile
}

#[tokio::test]
async fn a_changed_endpoint_asks_for_the_secret_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let profile = created(
        router,
        static_profile("https://minio-a.example:9000", Some(SECRET)),
    )
    .await;
    let uri = format!("{PROFILES}/{}", profile["id"].as_str().expect("id"));

    // Before, the stored secret went along to whatever host the update named.
    let (status, body) = common::put_json(
        router,
        &uri,
        static_profile("https://collector.example", None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "object_storage.secret_required", "{body}");

    // The same host keeps it: a rename needs no secret.
    let mut renamed = static_profile("https://minio-a.example:9000/", None);
    renamed["name"] = json!("Archive 2");
    let (status, body) = common::put_json(router, &uri, renamed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_secret"], true, "{body}");

    // A new host with the secret typed again is a plain change.
    let (status, body) = common::put_json(
        router,
        &uri,
        static_profile("https://minio-b.example", Some(SECRET)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["endpoint"], "https://minio-b.example", "{body}");
    assert_eq!(body["has_secret"], true, "{body}");
}

#[tokio::test]
async fn machine_credentials_reach_a_custom_endpoint_only_with_the_opt_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let mut ambient = json!({
        "name": "Machine",
        "provider": "s3",
        "endpoint": "https://minio.example:9000",
        "credential_source": "ambient",
    });
    let (status, body) = common::post_json(router, PROFILES, ambient.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["code"], "object_storage.ambient_endpoint_unconfirmed",
        "{body}"
    );

    ambient["ambient_custom_endpoint"] = json!(true);
    let profile = created(router, ambient).await;
    assert_eq!(profile["ambient_custom_endpoint"], true, "{profile}");
    // The provider's own service needs no yes.
    let own = created(
        router,
        json!({ "name": "AWS", "provider": "s3", "credential_source": "ambient" }),
    )
    .await;
    assert_eq!(own["ambient_custom_endpoint"], false, "{own}");
}

#[tokio::test]
async fn an_upload_target_names_an_existing_profile_and_only_its_bound_bucket() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let profile = created(
        router,
        static_profile("https://minio.example:9000", Some(SECRET)),
    )
    .await;
    let id = profile["id"].as_str().expect("id");
    let (status, settings) = common::get_json(router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");

    let missing = rd_core::ObjectStorageProfileId::new();
    for (remote, code) in [
        (
            format!("object-storage:{missing}/media-bucket"),
            Some("object_storage.upload_target_profile_unknown"),
        ),
        (
            format!("object-storage:{id}/other-bucket/in"),
            Some("object_storage.upload_target_bucket_refused"),
        ),
        (format!("object-storage:{id}/media-bucket/in"), None),
        (format!("object-storage:{id}"), None),
        ("archive:incoming".to_owned(), None),
    ] {
        let mut changed = settings.clone();
        changed["upload_remote"] = json!(remote);
        let (status, body) = common::put_json(router, "/api/v1/settings", changed).await;
        match code {
            Some(code) => {
                assert_eq!(status, StatusCode::BAD_REQUEST, "{remote}: {body}");
                assert_eq!(body["code"], code, "{remote}: {body}");
            }
            None => {
                assert_eq!(status, StatusCode::OK, "{remote}: {body}");
                // Saving the settings re-reads the login switch, which this harness keeps off
                // by hand.
                harness.state.auth.set_disabled(true);
            }
        }
    }
}

#[tokio::test]
async fn every_profile_change_leaves_an_audit_record() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let profile = created(
        router,
        static_profile("https://minio-a.example:9000", Some(SECRET)),
    )
    .await;
    let uri = format!("{PROFILES}/{}", profile["id"].as_str().expect("id"));
    let (status, body) = common::put_json(
        router,
        &uri,
        static_profile("https://minio-b.example", Some(SECRET)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = common::delete_json(router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(AuditAction::ObjectStorageProfileChanged),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    // Newest first.
    let changes: Vec<&str> = records
        .iter()
        .map(|record| record.details["change"].as_str())
        .collect();
    assert_eq!(changes, ["deleted", "updated", "created"]);
    let updated = &records[1];
    assert_eq!(
        updated.target_kind.as_deref(),
        Some("object_storage_profile")
    );
    assert_eq!(updated.details["endpoint"], "https://minio-b.example");
    let fields: Vec<&str> = updated.details["fields"].split(' ').collect();
    assert!(
        fields.contains(&"endpoint") && fields.contains(&"secret"),
        "{fields:?}"
    );
    let all = serde_json::to_string(&records.iter().map(|r| &r.details).collect::<Vec<_>>())
        .expect("json");
    assert!(
        !all.contains(SECRET),
        "a secret reached the audit log: {all}"
    );
}
