//! The S3 connector against a real service (RD-150-04): what the fixture in `tests/s3.rs` can
//! only imitate — the service's own signature check, its multipart ETags, `If-Match` with
//! `Range` on its validators and its answer to a wrong key.
//!
//! Ignored by default, and each test fails rather than passes when the service is not named.
//! CI runs them against RustFS in the `s3-live` job of `.github/workflows/ci.yml`; by hand:
//!
//! ```text
//! RD_S3_LIVE_ENDPOINT=http://127.0.0.1:9000 RD_S3_LIVE_BUCKET=rd-live \
//! RD_S3_LIVE_ACCESS_KEY=… RD_S3_LIVE_SECRET=… \
//!   cargo nextest run -p rd-object-storage --run-ignored only -E 'binary(s3_live)'
//! ```
//!
//! `RD_S3_LIVE_ENDPOINT` empty means AWS, `RD_S3_LIVE_REGION` defaults to `us-east-1`. The
//! bucket must exist and be writable: every run writes under a prefix of its own
//! (`rd-live-<uuid>/`) and deletes nothing, which a throw-away container does not need and a
//! lifecycle rule does elsewhere.

mod support;

use std::sync::Arc;

use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::NewObjectStorageProfile;
use rd_extract::{ObjectUpload, ObjectUploader, UploadReport};
use rd_scheduler::RunOutcome;
use tokio_util::sync::CancellationToken;

use support::{Harness, payload};

struct Live {
    endpoint: Option<String>,
    region: String,
    bucket: &'static str,
    access: String,
    secret: String,
}

fn live() -> Live {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let required = |name: &str| {
        variable(name).unwrap_or_else(|| {
            panic!(
                "{name} is not set: RD_S3_LIVE_BUCKET, RD_S3_LIVE_ACCESS_KEY and \
                 RD_S3_LIVE_SECRET name the service these tests run against"
            )
        })
    };
    Live {
        endpoint: variable("RD_S3_LIVE_ENDPOINT"),
        region: variable("RD_S3_LIVE_REGION").unwrap_or_else(|| "us-east-1".to_owned()),
        // The harness keeps the bucket for the life of the test process.
        bucket: Box::leak(required("RD_S3_LIVE_BUCKET").into_boxed_str()),
        access: required("RD_S3_LIVE_ACCESS_KEY"),
        secret: required("RD_S3_LIVE_SECRET"),
    }
}

async fn harness(live: &Live, secret: &str) -> Harness {
    Harness::start(
        NewObjectStorageProfile {
            name: "live".to_owned(),
            provider: ObjectStorageProvider::S3,
            addressing: ObjectAddressing::default_for(live.endpoint.as_deref()),
            endpoint: live.endpoint.clone(),
            region: Some(live.region.clone()),
            bucket: Some(live.bucket.to_owned()),
            credential_source: ObjectCredentialSource::Static,
            access_key_id: Some(live.access.clone()),
            account: None,
            secret_ref: None,
            session_token_ref: None,
            checksums: true,
            enabled: true,
        },
        Some(secret),
        "s3",
        live.bucket,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a writable S3 service, named through RD_S3_LIVE_*"]
async fn a_live_service_takes_a_multipart_upload_and_serves_it_back_resumably() {
    let live = live();
    let harness = harness(&live, &live.secret).await;
    assert!(
        harness
            .service
            .test_profile(&harness.profile)
            .await
            .expect("test")
            .is_none(),
        "the profile test must pass"
    );

    // Two parts of 16 MiB and a tail, each with its SHA-256 checksum.
    let prefix = format!("rd-live-{}", uuid::Uuid::now_v7());
    let body = payload(33 * 1024 * 1024 + 12_345);
    let directory = harness.directory.path().join("finished");
    tokio::fs::create_dir_all(&directory).await.expect("dir");
    tokio::fs::write(directory.join("big.bin"), &body)
        .await
        .expect("file");
    let files = vec!["big.bin".to_owned()];
    let destination = format!("{}/{prefix}", live.bucket);
    let report = harness
        .service
        .upload(
            &harness.profile.id.to_string(),
            ObjectUpload {
                owner: "live-package",
                package_name: "release",
                directory: &directory,
                files: &files,
                destination: &destination,
                progress: Arc::new(|_, _| {}),
                stop: CancellationToken::new(),
                bandwidth: rd_limits::ScopedLimiter::unlimited(),
            },
        )
        .await
        .expect("upload");
    assert_eq!(report, UploadReport::Verified { files });
    let key = format!("{prefix}/release/big.bin");

    // The prefix lists the object; the object itself answers with its size and ETag.
    let listing = harness
        .service
        .probe(&harness.link(&format!("{prefix}/")))
        .await
        .expect("probe")
        .expect("listing");
    let paths: Vec<&str> = listing
        .entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, vec!["release/big.bin"]);
    let single = harness
        .service
        .probe(&harness.link(&key))
        .await
        .expect("probe")
        .expect("listing");
    assert!(single.single_file);
    let entry = &single.entries[0];
    assert_eq!(
        entry.size.map(rd_core::ByteCount::get),
        Some(body.len() as u64)
    );
    let etag = entry.etag.clone().expect("the service names an ETag");

    // A whole download.
    let (file, package) = harness.queue(&key, "whole.bin").await;
    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination().join("whole.bin"))
        .await
        .expect("final");
    assert!(written == body, "the downloaded object differs");

    // A resume from 5 MiB, validated by the service's own (multipart) ETag.
    let (file, package) = harness.queue(&key, "resumed.bin").await;
    harness
        .partial(&file, &body[..5 * 1024 * 1024], body.len() as u64, &etag)
        .await;
    let outcome = harness.run(&file, &package).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination().join("resumed.bin"))
        .await
        .expect("final");
    assert!(written == body, "the resumed object differs");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a writable S3 service, named through RD_S3_LIVE_*"]
async fn a_live_service_refuses_a_wrong_secret_as_an_authentication_failure() {
    let live = live();
    let harness = harness(&live, "not-the-secret-of-this-access-key").await;
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test")
        .expect("a wrong secret must fail the test");
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_object_storage::error::AUTH_FAILED),
        "{failure:?}"
    );
}
