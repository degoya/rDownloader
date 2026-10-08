//! A full backup whose only copy lies elsewhere (RD-160-02, RD-160-03): a restore by run id
//! fetches the archive from the bucket the run wrote it to, and an upload to an rclone remote
//! carries the upload limit.
//!
//! The S3 endpoint is a small path-style fixture in the pattern of
//! `crates/rd-backup/tests/destinations.rs`: list, `HEAD`, `GET` and `PUT` of whole objects.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{Method, StatusCode, Uri},
    response::Response,
};
use rd_backup::restore::cutover::Layout;
use serde_json::json;

use crate::backup_destinations::{ready, run};
use crate::common;
use crate::full_restore::PASSPHRASE;

const BUCKET: &str = "backup-bucket";

type Objects = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn respond(status: StatusCode, headers: &[(&str, String)], body: impl Into<Body>) -> Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    builder
        .body(body.into())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn xml(status: StatusCode, body: String) -> Response {
    respond(
        status,
        &[("content-type", "application/xml".to_owned())],
        body,
    )
}

async fn s3(State(objects): State<Objects>, method: Method, uri: Uri, body: Bytes) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
    if bucket != BUCKET {
        return xml(
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchBucket</Code></Error>".to_owned(),
        );
    }
    let key = url::form_urlencoded::parse(format!("k={key}").as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    let headers = |length: usize| {
        vec![
            ("etag", "\"fixture\"".to_owned()),
            ("last-modified", "Mon, 28 Sep 2026 03:00:00 GMT".to_owned()),
            ("content-length", length.to_string()),
        ]
    };
    let stored = lock(&objects).get(&key).cloned();
    match (method, stored) {
        (Method::GET, _) if key.is_empty() => list(&objects, uri.query().unwrap_or("")),
        (Method::HEAD, Some(stored)) => {
            respond(StatusCode::OK, &headers(stored.len()), Body::empty())
        }
        (Method::GET, Some(stored)) => respond(StatusCode::OK, &headers(stored.len()), stored),
        (Method::HEAD | Method::GET, None) => xml(
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchKey</Code></Error>".to_owned(),
        ),
        (Method::PUT, _) => {
            lock(&objects).insert(key, body.to_vec());
            respond(
                StatusCode::OK,
                &[("etag", "\"fixture\"".to_owned())],
                Body::empty(),
            )
        }
        _ => xml(
            StatusCode::BAD_REQUEST,
            "<Error><Code>NotImplemented</Code></Error>".to_owned(),
        ),
    }
}

fn list(objects: &Objects, query: &str) -> Response {
    let prefix = url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "prefix")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    let contents: String = lock(objects)
        .iter()
        .filter(|(key, _)| {
            key.strip_prefix(prefix.as_str())
                .is_some_and(|rest| !rest.contains('/'))
        })
        .map(|(key, body)| {
            format!(
                "<Contents><Key>{key}</Key><LastModified>2026-09-28T03:00:00.000Z</LastModified>\
                 <ETag>\"fixture\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>\
                 </Contents>",
                body.len()
            )
        })
        .collect();
    xml(
        StatusCode::OK,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix>{prefix}</Prefix><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>\
             {contents}</ListBucketResult>"
        ),
    )
}

/// The fixture's objects and its endpoint.
async fn bucket() -> (Objects, String) {
    let objects = Objects::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let app = Router::new()
        .fallback(s3)
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(objects.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (objects, format!("http://{address}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_kept_only_in_a_bucket_restores_by_its_id() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (objects, endpoint) = bucket().await;
    let secret = harness
        .secrets
        .put_string("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned())
        .await
        .expect("secret");
    let profile = harness
        .database
        .create_object_storage_profile(rd_db::NewObjectStorageProfile {
            name: "backups".to_owned(),
            provider: rd_core::ObjectStorageProvider::S3,
            endpoint: Some(endpoint),
            region: Some("us-east-1".to_owned()),
            bucket: Some(BUCKET.to_owned()),
            addressing: rd_core::ObjectAddressing::Path,
            credential_source: rd_core::ObjectCredentialSource::Static,
            access_key_id: Some("AKIDEXAMPLE".to_owned()),
            account: None,
            secret_ref: Some(secret),
            session_token_ref: None,
            checksums: false,
            enabled: true,
            ambient_custom_endpoint: false,
        })
        .await
        .expect("profile");
    ready(&harness).await;
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/destinations",
        json!({
            "kind": "object_storage",
            "profile_id": profile.id.to_string(),
            "prefix": format!("{BUCKET}/rd"),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let written = run(&harness).await;
    assert_eq!(
        written.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        written.error_detail
    );
    let name = written.archive_name.clone().expect("archive");
    assert!(
        written
            .destinations
            .iter()
            .all(|row| row.kind == "object_storage"),
        "the bucket holds the only copy"
    );
    assert!(lock(&objects).contains_key(&format!("rd/{name}")));

    let preview = json!({ "source": { "run_id": written.id }, "passphrase": PASSPHRASE });
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/preview",
        preview.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["parts"]
            .as_array()
            .is_some_and(|parts| !parts.is_empty()),
        "{body}"
    );
    let fetched = Layout::new(harness.database.path())
        .work()
        .join(format!("run-{}", written.id))
        .join(&name);
    assert!(
        fetched.is_file(),
        "the archive was fetched into the work folder"
    );

    // The next step of the same run reads the fetched copy; the bucket is not asked again.
    lock(&objects).clear();
    let (status, body) =
        common::post_json(&harness.router, "/api/v1/backups/restore/preview", preview).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A stand-in for rclone that appends every call's arguments to `calls.log` and keeps the
/// remote `stub:` in `remote/`, both beside it; exit status 3 is rclone's "no such folder".
#[cfg(unix)]
fn recording_rclone(directory: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    const SCRIPT: &str = r#"#!/bin/sh
echo "$*" >> 'DIR/calls.log'
verb="$1"; shift
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
map() { case "$1" in stub:*) printf '%s/%s' 'DIR/remote' "${1#stub:}" ;; *) printf '%s' "$1" ;; esac; }
case "$verb" in
  copyto) to=$(map "$2"); mkdir -p "$(dirname "$to")" && cp "$(map "$1")" "$to" ;;
  moveto) mv "$(map "$1")" "$(map "$2")" ;;
  deletefile) rm "$(map "$1")" ;;
  lsjson) folder=$(map "$1"); [ -d "$folder" ] || exit 3
          printf '['; sep=''
          for file in "$folder"/*; do
            [ -f "$file" ] || continue
            name=$(basename "$file"); size=$(wc -c < "$file" | tr -d ' ')
            printf '%s{"Path":"%s","Name":"%s","Size":%s,"IsDir":false}' "$sep" "$name" "$name" "$size"
            sep=','
          done
          printf ']' ;;
  *) exit 1 ;;
esac
"#;
    let script = directory.join("rclone");
    std::fs::write(
        &script,
        SCRIPT.replace("DIR", &directory.display().to_string()),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

#[cfg(unix)]
#[tokio::test]
async fn an_upload_to_an_rclone_remote_carries_the_upload_limit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let rclone = recording_rclone(directory.path());
    let (status, mut settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["upload_limit_bytes_per_second"] = json!("500000");
    settings["rclone_executable"] = json!(rclone.display().to_string());
    // The harness stands in for an installation without an admin password, and saving the
    // settings re-applies that switch.
    settings["admin_login_disabled"] = json!(true);
    let (status, saved) = common::put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    ready(&harness).await;
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/backups/destinations",
        json!({ "kind": "rclone", "remote": "stub:backups" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let written = run(&harness).await;
    assert_eq!(
        written.state,
        rd_core::BackupRunState::Succeeded,
        "{:?}",
        written.error_detail
    );
    let calls = std::fs::read_to_string(directory.path().join("calls.log")).expect("calls");
    let upload = calls
        .lines()
        .find(|line| line.starts_with("copyto"))
        .expect("an upload");
    assert!(upload.contains("--bwlimit 500000B --"), "{calls}");
    assert!(
        calls
            .lines()
            .filter(|line| !line.starts_with("copyto"))
            .all(|line| !line.contains("--bwlimit")),
        "only the upload is limited: {calls}"
    );
}
