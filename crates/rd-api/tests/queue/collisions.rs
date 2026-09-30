//! Collision policies, prompts, duplicates and dedupe links over REST (RD-150-01, RD-150-02).

use crate::common;

use axum::http::StatusCode;
use rd_core::{DownloadFile, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec};

pub(crate) const SOURCE: &str = "https://example.invalid/release/file.bin";

/// A paused package with one file named `file.bin` from `source`, below `base`.
pub(crate) async fn paused_download(
    harness: &common::Harness,
    base: &std::path::Path,
    name: &str,
    source: &str,
) -> DownloadFile {
    let (_package, files) = harness
        .scheduler
        .enqueue_package(
            PackageSpec {
                name: name.to_owned(),
                destination: base.to_path_buf(),
                category_id: None,
                priority: rd_core::DownloadPriority::default(),
                password: None,
                start_paused: true,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            },
            vec![FileSpec {
                source: source.parse().expect("url"),
                file_name: "file.bin".to_owned(),
                size: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::default(),
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                skipped: false,
                enrichment: Vec::new(),
                secret_fragment: None,
                source_set: None,
            }],
        )
        .await
        .expect("enqueue");
    files.into_iter().next().expect("one file")
}

/// Finishes a paused download on `bytes` written to `path`, indexed like the worker does.
pub(crate) async fn finished(
    harness: &common::Harness,
    file: &DownloadFile,
    path: &std::path::Path,
    bytes: &[u8],
) {
    tokio::fs::create_dir_all(path.parent().expect("folder"))
        .await
        .expect("folder");
    tokio::fs::write(path, bytes).await.expect("file");
    for state in [DownloadState::Downloading, DownloadState::Verifying] {
        harness
            .database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
    let digest = rd_files::compute_checksum(path, rd_core::ChecksumAlgorithm::Sha256)
        .await
        .expect("digest");
    harness
        .database
        .complete_download(
            file.id,
            file.file_name.clone(),
            Some(rd_core::ExpectedChecksum {
                algorithm: digest.algorithm,
                value: digest.value.clone(),
            }),
        )
        .await
        .expect("complete");
    harness
        .database
        .index_content(
            file.id,
            "sha256".to_owned(),
            digest.value,
            u64::try_from(bytes.len()).expect("length"),
            path.to_string_lossy().into_owned(),
        )
        .await
        .expect("index");
}

#[tokio::test]
async fn a_package_policy_is_set_read_and_inherited_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let file = paused_download(&harness, &directory.path().join("storage"), "one", SOURCE).await;
    let route = format!("/api/v1/packages/{}/collision-policy", file.package_id);

    let (status, view) = common::get_json(&harness.router, &route).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["global"], "rename");
    assert_eq!(view["effective"]["policy"], "rename");
    assert_eq!(view["effective"]["source"], "global");

    let (status, view) = common::put_json(
        &harness.router,
        &route,
        serde_json::json!({ "policy": "ask" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["own"], "ask");
    assert_eq!(view["effective"]["source"], "package");

    let (status, all) = common::get_json(&harness.router, "/api/v1/collision-policies").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all["packages"][0]["policy"], "ask");

    let (status, view) = common::put_json(
        &harness.router,
        &route,
        serde_json::json!({ "policy": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(view["own"].is_null());
    assert_eq!(view["effective"]["source"], "global");

    let (status, refused) = common::put_json(
        &harness.router,
        &format!(
            "/api/v1/categories/{}/collision-policy",
            rd_core::CategoryId::new()
        ),
        serde_json::json!({ "policy": "skip" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["code"], "category.not_found");

    let (status, settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["storage_collision_policy"], "rename");
}

/// A prompt is listed, answered once, audited, and the download goes on; a download with no
/// question refuses an answer.
#[tokio::test]
async fn a_prompt_is_answered_audited_and_released() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let file = paused_download(&harness, &directory.path().join("storage"), "one", SOURCE).await;
    harness
        .database
        .transition_download(file.id, DownloadState::Downloading)
        .await
        .expect("started");
    harness
        .database
        .open_collision_prompt(rd_db::NewCollisionPrompt {
            download_id: file.id,
            target_name: "file.bin".to_owned(),
            phase: rd_core::CollisionPhase::BeforeTransfer,
            existing_bytes: Some(3),
        })
        .await
        .expect("prompt");
    harness
        .database
        .block_download(file.id, rd_scheduler::BlockReason::CollisionAsk.as_str())
        .await
        .expect("blocked");

    let (status, prompts) = common::get_json(&harness.router, "/api/v1/collision-prompts").await;
    assert_eq!(status, StatusCode::OK, "{prompts}");
    assert_eq!(prompts[0]["download_id"], file.id.to_string());
    assert_eq!(prompts[0]["package_name"], "one");
    assert_eq!(prompts[0]["existing_bytes"], 3);

    let route = format!("/api/v1/downloads/{}/collision-decision", file.id);
    let (status, answer) = common::post_json(
        &harness.router,
        &route,
        serde_json::json!({ "decision": "overwrite" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["code"], "collision.decided");
    let row = harness
        .database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("row");
    assert_ne!(
        row.state,
        DownloadState::Blocked,
        "the answer did not release the download"
    );
    let prompt = harness
        .database
        .collision_prompt(file.id)
        .await
        .expect("read")
        .expect("kept until carried out");
    assert_eq!(prompt.decision, Some(rd_core::CollisionDecision::Overwrite));
    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::CollisionDecided),
            limit: 10,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].details.get("decision").map(String::as_str),
        Some("overwrite")
    );

    let other = paused_download(&harness, &directory.path().join("storage"), "two", SOURCE).await;
    let (status, refused) = common::post_json(
        &harness.router,
        &format!("/api/v1/downloads/{}/collision-decision", other.id),
        serde_json::json!({ "decision": "skip" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "collision.no_prompt");
}

/// Source and content duplicates come back in separate lists, and an identical finished file
/// can be replaced by a link to its original.
#[tokio::test]
async fn duplicates_are_explained_apart_and_an_identical_file_is_linked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let storage = directory.path().join("storage");
    let first = paused_download(&harness, &storage, "one", SOURCE).await;
    // The same source, spelled differently: another scheme and a fragment.
    let second = paused_download(
        &harness,
        &storage,
        "two",
        "http://example.invalid/release/file.bin#copy",
    )
    .await;
    let unrelated =
        paused_download(&harness, &storage, "three", "https://other.invalid/x.bin").await;

    let (status, report) = common::get_json(
        &harness.router,
        &format!("/api/v1/downloads/{}/duplicates", first.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let source = report["source"].as_array().expect("source");
    assert_eq!(source.len(), 1, "{report}");
    assert_eq!(source[0]["download_id"], second.id.to_string());
    assert_eq!(source[0]["location"], "queue");
    assert!(report["content"].as_array().expect("content").is_empty());
    assert!(report["content_basis"].is_null(), "nothing is hashed yet");

    let first_path = storage.join("one").join("file.bin");
    let second_path = storage.join("three").join("file.bin");
    finished(&harness, &first, &first_path, b"identical bytes").await;
    finished(&harness, &unrelated, &second_path, b"identical bytes").await;

    let (status, report) = common::get_json(
        &harness.router,
        &format!("/api/v1/downloads/{}/duplicates", unrelated.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(
        report["source"].as_array().expect("source").is_empty(),
        "another address is no source duplicate: {report}"
    );
    assert_eq!(report["content_basis"], "verified_hash");
    let content = report["content"].as_array().expect("content");
    assert_eq!(content.len(), 1, "{report}");
    assert_eq!(content[0]["download_id"], first.id.to_string());
    assert_eq!(content[0]["missing"], false);

    let route = format!("/api/v1/downloads/{}/dedupe", unrelated.id);
    let (status, refused) = common::post_json(
        &harness.router,
        &route,
        serde_json::json!({ "original_download_id": first.id, "mode": "reflink" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "storage.reflink_unsupported");

    let (status, linked) = common::post_json(
        &harness.router,
        &route,
        serde_json::json!({ "original_download_id": first.id, "mode": "hardlink" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["freed_bytes"], 15);
    assert_eq!(
        tokio::fs::read(&second_path).await.expect("read"),
        b"identical bytes"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&first_path).expect("meta").ino(),
            std::fs::metadata(&second_path).expect("meta").ino(),
            "the duplicate is a link to the original now"
        );
    }

    let (status, history) = common::get_json(&harness.router, "/api/v1/storage/operations").await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history[0]["kind"], "dedupe");
    assert_eq!(history[0]["state"], "completed");
    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::DuplicateLinked),
            limit: 10,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(records.len(), 1);

    let (status, lookup) = common::post_json(
        &harness.router,
        "/api/v1/duplicates/lookup",
        serde_json::json!({ "urls": [SOURCE, "https://nowhere.invalid/a"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{lookup}");
    assert_eq!(lookup[0]["queue"].as_array().expect("queue").len(), 2);
    assert!(lookup[1]["queue"].as_array().expect("queue").is_empty());
    let (status, refused) = common::post_json(
        &harness.router,
        "/api/v1/duplicates/lookup",
        serde_json::json!({ "urls": ["not a url"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "duplicates.url_invalid");
}

#[tokio::test]
async fn every_runner_declares_its_reuse_and_the_index_can_be_checked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;

    let (status, reuse) = common::get_json(&harness.router, "/api/v1/storage/reuse").await;
    assert_eq!(status, StatusCode::OK, "{reuse}");
    assert_eq!(reuse[0]["kind"], "http");
    assert_eq!(reuse[0]["capability"]["applies_collision_policy"], true);

    let (status, support) = common::get_json(&harness.router, "/api/v1/storage/link-support").await;
    assert_eq!(status, StatusCode::OK, "{support}");
    for entry in support.as_array().expect("entries") {
        assert_eq!(entry["reflink"], false, "{entry}");
    }

    let (status, check) = common::post_json(
        &harness.router,
        "/api/v1/storage/content-index/check",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{check}");
    assert_eq!(check["missing"], 0);

    let (status, refused) =
        common::get_json(&harness.router, "/api/v1/storage/operations?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "storage.operations_limit_invalid");
}
