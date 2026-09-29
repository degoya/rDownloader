//! Every destination meets the same contract (RD-160-02): the local folder, a folder of an S3
//! bucket and an rclone remote.
//!
//! The contract is what the run, retention and verification rely on: a stored archive is a
//! copy (the staged file stays for the next destination), it is listed under its name with its
//! size, a taken name is refused and the archive under it untouched, it comes back byte for
//! byte, it can be removed once, and no name outside the plain `*.rdbackup` file names reaches
//! the destination at all. The S3 endpoint is a small fixture in the pattern of
//! `rd-object-storage/tests/s3.rs`; rclone is a shell script standing in for the binary.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{Method, StatusCode, Uri},
    response::Response,
};
use rd_backup::{
    BackupDestination, DestinationConfig, DestinationContext, DestinationError, ListedArchive,
    LocalFolder,
};
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::{Database, NewObjectStorageProfile};
use rd_object_storage::ObjectStorageService;
use tokio::sync::RwLock;

const NAME: &str = "rdownloader-backup-0a1b2c3d-20260928T030000Z.rdbackup";
const BUCKET: &str = "backup-bucket";

fn payload(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 241) as u8).collect()
}

/// The contract, run against one destination with `scratch` as its local side.
async fn contract(destination: &dyn BackupDestination, scratch: &Path) {
    let archive = scratch.join("staged.rdbackup");
    let body = payload(200_000);
    std::fs::write(&archive, &body).expect("stage");
    assert!(destination.list().await.expect("list").is_empty());

    let stored = destination.store(&archive, NAME).await.expect("store");
    assert!(stored.location.contains(NAME), "{}", stored.location);
    assert!(archive.exists(), "a store is a copy");
    assert_eq!(
        destination.list().await.expect("list"),
        vec![ListedArchive {
            name: NAME.to_owned(),
            size: body.len() as u64,
        }]
    );

    // A taken name is refused, and the archive under it stays what it was.
    let other = scratch.join("other.rdbackup");
    std::fs::write(&other, b"something else").expect("other");
    let taken = destination.store(&other, NAME).await.expect_err("taken");
    assert!(matches!(taken, DestinationError::NameTaken(_)), "{taken:?}");

    let back = scratch.join("back.rdbackup");
    let size = destination.fetch(NAME, &back).await.expect("fetch");
    assert_eq!(size, body.len() as u64);
    assert_eq!(std::fs::read(&back).expect("read back"), body);

    for bad in [
        "../escape.rdbackup",
        "sub/inner.rdbackup",
        "notes.txt",
        ".hidden.rdbackup",
        "",
    ] {
        let refused = destination.store(&archive, bad).await.expect_err(bad);
        assert!(
            matches!(refused, DestinationError::InvalidName(_)),
            "{bad}: {refused:?}"
        );
        assert!(matches!(
            destination.remove(bad).await,
            Err(DestinationError::InvalidName(_))
        ));
        assert!(matches!(
            destination.fetch(bad, &scratch.join("never")).await,
            Err(DestinationError::InvalidName(_))
        ));
    }
    let missing = "rdownloader-backup-0a1b2c3d-20200101T000000Z.rdbackup";
    assert!(matches!(
        destination.fetch(missing, &scratch.join("missing")).await,
        Err(DestinationError::NotFound(_))
    ));

    destination.remove(NAME).await.expect("remove");
    assert!(destination.list().await.expect("list").is_empty());
    assert!(matches!(
        destination.remove(NAME).await,
        Err(DestinationError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_local_folder_meets_the_contract() {
    let directory = tempfile::tempdir().expect("temp");
    let folder = LocalFolder::open(&directory.path().join("nas"))
        .await
        .expect("folder");
    // Whatever else lies in the folder is not an archive to it.
    std::fs::write(directory.path().join("nas/notes.txt"), b"mine").expect("foreign");
    contract(&folder, directory.path()).await;
    assert!(directory.path().join("nas/notes.txt").exists());
}

// --- A folder of an S3 bucket -------------------------------------------------------------

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

fn not_found() -> Response {
    xml(
        StatusCode::NOT_FOUND,
        "<Error><Code>NoSuchKey</Code></Error>".to_owned(),
    )
}

/// Path-style S3: `GET /bucket?list-type=2`, `GET|HEAD|PUT /bucket/key`, `POST /bucket?delete`.
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
    let query: BTreeMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let headers = |length: usize| {
        vec![
            ("etag", "\"fixture\"".to_owned()),
            ("last-modified", "Mon, 28 Sep 2026 03:00:00 GMT".to_owned()),
            ("content-length", length.to_string()),
        ]
    };
    match method {
        Method::GET if key.is_empty() => list(&objects, &query),
        Method::HEAD => match lock(&objects).get(&key) {
            Some(stored) => respond(StatusCode::OK, &headers(stored.len()), Body::empty()),
            None => not_found(),
        },
        Method::GET => match lock(&objects).get(&key).cloned() {
            Some(stored) => respond(StatusCode::OK, &headers(stored.len()), stored),
            None => not_found(),
        },
        Method::PUT => {
            lock(&objects).insert(key, body.to_vec());
            respond(
                StatusCode::OK,
                &[("etag", "\"fixture\"".to_owned())],
                Body::empty(),
            )
        }
        Method::POST if query.contains_key("delete") => {
            let text = String::from_utf8_lossy(&body).into_owned();
            let mut deleted = String::new();
            for part in text.split("<Key>").skip(1) {
                let Some((name, _)) = part.split_once("</Key>") else {
                    continue;
                };
                lock(&objects).remove(name);
                deleted.push_str(&format!("<Deleted><Key>{name}</Key></Deleted>"));
            }
            xml(
                StatusCode::OK,
                format!("<DeleteResult>{deleted}</DeleteResult>"),
            )
        }
        _ => xml(
            StatusCode::BAD_REQUEST,
            "<Error><Code>NotImplemented</Code></Error>".to_owned(),
        ),
    }
}

fn list(objects: &Objects, query: &BTreeMap<String, String>) -> Response {
    let prefix = query.get("prefix").cloned().unwrap_or_default();
    let delimited = query.contains_key("delimiter");
    let mut contents = String::new();
    let mut folders = std::collections::BTreeSet::new();
    for (key, body) in lock(objects).iter() {
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };
        if delimited && let Some((folder, _)) = rest.split_once('/') {
            folders.insert(format!("{prefix}{folder}/"));
            continue;
        }
        contents.push_str(&format!(
            "<Contents><Key>{key}</Key><LastModified>2026-09-28T03:00:00.000Z</LastModified>\
             <ETag>\"fixture\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>\
             </Contents>",
            body.len()
        ));
    }
    let prefixes: String = folders
        .iter()
        .map(|folder| format!("<CommonPrefixes><Prefix>{folder}</Prefix></CommonPrefixes>"))
        .collect();
    xml(
        StatusCode::OK,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix>{prefix}</Prefix><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>\
             {contents}{prefixes}</ListBucketResult>"
        ),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_of_a_bucket_meets_the_contract() {
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

    let directory = tempfile::tempdir().expect("temp");
    let database = Database::open(directory.path().join("backup.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let secret_ref = secrets
        .put_string("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned())
        .await
        .expect("secret");
    let profile = database
        .create_object_storage_profile(NewObjectStorageProfile {
            name: "backups".to_owned(),
            provider: ObjectStorageProvider::S3,
            endpoint: Some(format!("http://{address}")),
            region: Some("us-east-1".to_owned()),
            bucket: Some(BUCKET.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: ObjectCredentialSource::Static,
            access_key_id: Some("AKIDEXAMPLE".to_owned()),
            account: None,
            secret_ref: Some(secret_ref),
            session_token_ref: None,
            checksums: false,
            enabled: true,
        })
        .await
        .expect("profile");
    let context = DestinationContext {
        object_storage: ObjectStorageService::new(
            database.clone(),
            secrets,
            Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        ),
        rclone_executable: None,
        vendor_directory: None,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
    };
    // An object beside the folder and one below it are not the folder's.
    lock(&objects).insert("elsewhere.rdbackup".to_owned(), b"x".to_vec());
    lock(&objects).insert("rd/deeper/inner.rdbackup".to_owned(), b"x".to_vec());
    let config = DestinationConfig::ObjectStorage {
        profile_id: profile.id.to_string(),
        prefix: format!("{BUCKET}/rd"),
    };
    let destination = config.open(&context).await.expect("open");
    assert_eq!(destination.kind(), "object_storage");
    contract(destination.as_ref(), directory.path()).await;
    assert!(lock(&objects).contains_key("elsewhere.rdbackup"));
    assert!(lock(&objects).contains_key("rd/deeper/inner.rdbackup"));

    // A deleted profile is a configuration fault, not an outage to retry.
    let gone = DestinationConfig::ObjectStorage {
        profile_id: "999999".to_owned(),
        prefix: BUCKET.to_owned(),
    };
    let refused = gone.open(&context).await.err().expect("no profile");
    assert!(!refused.is_transient());
    assert_eq!(refused.code(), rd_object_storage::FOLDER_PROFILE_MISSING);
}

// --- An rclone remote ---------------------------------------------------------------------

/// A stand-in for rclone that maps `stub:<path>` into `root` and knows the four verbs the
/// destination uses, with rclone's exit status 3/4 for what is not there. With `down` set it
/// fails every call the way an unreachable remote does.
#[cfg(unix)]
fn stub_rclone(directory: &Path, root: &Path, down: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join(if down { "rclone-down" } else { "rclone" });
    let body = if down {
        "#!/bin/sh\necho 'Failed to create file system: dial tcp: connection refused' >&2\nexit 1\n"
            .to_owned()
    } else {
        format!(
            r#"#!/bin/sh
root='{root}'
verb="$1"; shift
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
map() {{ case "$1" in stub:*) printf '%s/%s' "$root" "${{1#stub:}}" ;; *) printf '%s' "$1" ;; esac; }}
case "$verb" in
  copyto) from=$(map "$1"); to=$(map "$2"); [ -f "$from" ] || exit 4
          mkdir -p "$(dirname "$to")" && cp "$from" "$to" ;;
  moveto) from=$(map "$1"); to=$(map "$2"); [ -f "$from" ] || exit 4; mv "$from" "$to" ;;
  deletefile) file=$(map "$1"); [ -f "$file" ] || exit 4; rm "$file" ;;
  lsjson) folder=$(map "$1"); [ -d "$folder" ] || exit 3
          printf '['; sep=''
          for file in "$folder"/* "$folder"/.*; do
            [ -f "$file" ] || continue
            name=$(basename "$file"); size=$(wc -c < "$file" | tr -d ' ')
            printf '%s{{"Path":"%s","Name":"%s","Size":%s,"IsDir":false}}' "$sep" "$name" "$name" "$size"
            sep=','
          done
          printf ']' ;;
  *) echo "unknown verb $verb" >&2; exit 1 ;;
esac
"#,
            root = root.display()
        )
    };
    std::fs::write(&script, body).expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

#[cfg(unix)]
#[tokio::test]
async fn an_rclone_remote_meets_the_contract() {
    let directory = tempfile::tempdir().expect("temp");
    let root = directory.path().join("remote");
    std::fs::create_dir_all(root.join("backups")).expect("remote");
    std::fs::write(root.join("backups/holiday.jpg"), b"not mine").expect("foreign");
    let tool = stub_rclone(directory.path(), &root, false);
    let destination = rd_backup::remote::RcloneDestination::new(rd_extract::RcloneRemote::new(
        tool,
        "stub:backups",
        Some(1_000_000),
    ));
    assert_eq!(destination.describe(), "stub:backups");
    contract(&destination, directory.path()).await;
    assert!(root.join("backups/holiday.jpg").exists());
    // The temporary upload name never stays behind.
    assert!(!root.join(format!("backups/{NAME}.partial")).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreachable_rclone_remote_is_an_outage_worth_retrying() {
    let directory = tempfile::tempdir().expect("temp");
    let tool = stub_rclone(directory.path(), directory.path(), true);
    let destination = rd_backup::remote::RcloneDestination::new(rd_extract::RcloneRemote::new(
        tool,
        "stub:backups",
        None,
    ));
    let archive = directory.path().join("staged.rdbackup");
    std::fs::write(&archive, b"sealed").expect("stage");
    let failed = destination.store(&archive, NAME).await.expect_err("down");
    assert!(failed.is_transient(), "{failed:?}");
    assert_eq!(failed.code(), "backup.rclone_failed");
    assert!(failed.to_string().contains("connection refused"));
}

#[tokio::test]
async fn an_rclone_remote_without_rclone_is_named_as_such() {
    let directory = tempfile::tempdir().expect("temp");
    let database = Database::open(directory.path().join("backup.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let context = DestinationContext {
        object_storage: ObjectStorageService::new(
            database,
            secrets,
            Arc::new(RwLock::new(rd_core::RemoteSettings::default())),
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        ),
        rclone_executable: Some(
            directory
                .path()
                .join("no-rclone-here")
                .display()
                .to_string(),
        ),
        vendor_directory: Some(directory.path().display().to_string()),
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
    };
    let config = DestinationConfig::Rclone {
        remote: "nas:backups".to_owned(),
    };
    // The shape is fine, so saving it is; a run then names what is missing.
    config.validate(&context).await.expect("valid");
    let missing = config.open(&context).await.err().expect("no rclone");
    assert_eq!(missing.code(), "backup.rclone_missing");
    assert!(!missing.is_transient());
    let refused = DestinationConfig::Rclone {
        remote: "-nas".to_owned(),
    }
    .validate(&context)
    .await
    .expect_err("not a remote");
    assert_eq!(refused.code(), rd_backup::remote::RCLONE_REMOTE_INVALID);
}
