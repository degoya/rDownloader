//! Exporting packages as a link file and taking it back in, and re-resolving downloads with the
//! plugin installed now (RD-1210-01).
//!
//! The point of both: a download stays bound to the resolver version that first resolved it, so
//! a faulty version keeps its jobs. An exported file carries no binding at all, and re-resolving
//! drops it in place.

use crate::common;

use axum::http::{StatusCode, header};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::{
    delete_json, get_json, parked_harness, post_json, request_to, send_raw,
    wait_for_candidates_ready,
};
use serde_json::{Value, json};

const PLUGIN: &str = "019d0000-0000-7000-8000-000000000108";
const OLD_VERSION: &str = "0.0.1";
const ADDRESS: &str = "https://ddownload.example/abc123/holiday.part1.rar";
/// `rd_collector::MAX_RDLINKS_LINKS`, which this binary does not link.
const MAX_LINKS: usize = 2_000;

/// A package of one download in `state`, pinned to [`OLD_VERSION`] as resolving pins it.
async fn pinned_package(
    database: &rd_db::Database,
    name: &str,
    state: rd_core::DownloadState,
) -> (rd_core::PackageId, rd_core::DownloadId) {
    use rd_core::DownloadState::{Completed, Downloading, Paused, Queued, Resolving, Verifying};
    let finished = state == Completed;
    let state = if matches!(state, Queued | Paused) {
        state
    } else {
        Queued
    };
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: name.to_owned(),
            destination: format!("downloads/{name}"),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .update_packages(
            vec![package_id],
            rd_db::PackageChange {
                category: None,
                priority: None,
                name: None,
                password: Some(Some("archive secret".to_owned())),
                postprocess_level: None,
                script: None,
            },
        )
        .await
        .expect("password");
    let download = database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: ADDRESS.parse().expect("URL"),
            file_name: "holiday.part1.rar".to_owned(),
            total_bytes: Some(rd_core::ByteCount::new(1_048_576).expect("size")),
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: state,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: Some("part1".to_owned()),
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .claim_resolver_pin(
            download.id,
            rd_core::ResolverPin {
                plugin_id: PLUGIN.parse().expect("plugin id"),
                version: OLD_VERSION.to_owned(),
            },
        )
        .await
        .expect("pin");
    if finished {
        for step in [Resolving, Downloading, Verifying, Completed] {
            database
                .transition_download(download.id, step)
                .await
                .expect("transition");
        }
    }
    (package_id, download.id)
}

fn import_body(file_name: &str, content: &[u8], passphrase: Option<&str>) -> Value {
    let mut body = json!({ "file_name": file_name, "content": STANDARD.encode(content) });
    if let Some(passphrase) = passphrase {
        body["passphrase"] = passphrase.into();
    }
    body
}

/// The export carries the links and no binding; brought back, they are fresh LinkGrabber links,
/// and queued again they start without a pin, so the next start resolves with the plugin
/// installed then.
#[tokio::test]
async fn an_exported_package_comes_back_without_its_plugin_binding() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (package_id, old_download) =
        pinned_package(&harness.database, "Holiday", rd_core::DownloadState::Paused).await;

    let (status, file) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "package_ids": [package_id], "format": "rdlinks" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["format"], "rdownloader-links/1");
    let package = &file["packages"][0];
    assert_eq!(package["name"], "Holiday");
    assert_eq!(package["password"], "archive secret");
    assert_eq!(package["links"][0]["url"], ADDRESS);
    assert_eq!(package["links"][0]["file_name"], "holiday.part1.rar");
    assert_eq!(package["links"][0]["size"], 1_048_576);
    assert_eq!(package["links"][0]["mirror_group"], "part1");
    let text = file.to_string();
    assert!(
        !text.contains(PLUGIN) && !text.contains(OLD_VERSION),
        "{text}"
    );
    // The owner's case: export, delete, import. The paused file still waits, so the delete is
    // the forced one the download list asks for.
    let (status, body) = delete_json(
        &harness.router,
        &format!("/api/v1/packages/{package_id}?force=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("holiday.rdlinks", text.as_bytes(), None),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["format"], "rdlinks");
    assert_eq!(imported["candidates"][0]["url"], ADDRESS);
    assert_eq!(imported["packages"][0]["name"], "Holiday");
    assert_eq!(imported["packages"][0]["password"], "archive secret");

    wait_for_candidates_ready(&harness.router).await;
    let collector_id = imported["packages"][0]["id"].clone();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": [collector_id], "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let downloads = harness.database.list_downloads().await.expect("downloads");
    let fresh = downloads
        .iter()
        .find(|download| download.source.as_str() == ADDRESS)
        .expect("the imported link queued");
    assert_ne!(fresh.id, old_download);
    assert_eq!(
        harness.database.resolver_pin(fresh.id).await.expect("pin"),
        None,
        "no binding came along: the next start resolves with the plugin installed then"
    );
}

#[tokio::test]
async fn a_sealed_export_opens_only_with_its_passphrase() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (package_id, _) =
        pinned_package(&harness.database, "Sealed", rd_core::DownloadState::Queued).await;

    let (status, file) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "package_ids": [package_id], "format": "rdlinks", "passphrase": "correct horse" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let text = file.to_string();
    assert!(file["encryption"].is_object(), "{text}");
    assert!(!text.contains("ddownload") && !text.contains("archive secret"));

    for (passphrase, code) in [
        (None, "rdlinks.passphrase_required"),
        (Some("wrong horse"), "rdlinks.passphrase_invalid"),
    ] {
        let (status, body) = post_json(
            &harness.router,
            "/api/v1/containers/import",
            import_body("sealed.rdlinks", text.as_bytes(), passphrase),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], code);
    }
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("sealed.rdlinks", text.as_bytes(), Some("correct horse")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["candidates"][0]["url"], ADDRESS);
}

/// An edited file is refused whole, and the limits hold on both sides.
#[tokio::test]
async fn a_manipulated_file_and_the_limits_are_refused_with_their_codes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    let local = json!({ "format": "rdownloader-links/1", "packages": [
        { "name": "x", "links": [{ "url": "file:///etc/passwd" }] }
    ]});
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("x.rdlinks", local.to_string().as_bytes(), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "rdlinks.file_invalid");

    let links: Vec<Value> = (0..=MAX_LINKS)
        .map(|index| json!({ "url": format!("https://many.example/{index}") }))
        .collect();
    let many = json!({ "format": "rdownloader-links/1", "packages": [{ "links": links }] });
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("many.rdlinks", many.to_string().as_bytes(), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "rdlinks.file_invalid");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "format": "rdlinks" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "export.nothing_selected");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "all": true, "format": "crawljob", "passphrase": "correct horse" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "export.passphrase_unsupported");
}

/// The crawljob is plain text, an attachment, and carries addresses, name and password only.
#[tokio::test]
async fn a_crawljob_export_is_an_attachment_jdownloader_reads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    pinned_package(
        &harness.database,
        "Holiday",
        rd_core::DownloadState::Completed,
    )
    .await;

    let request = request_to("POST", "/api/v1/packages/export")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({ "all": true, "format": "crawljob" }).to_string(),
        ))
        .expect("request");
    let (status, headers, bytes) = send_raw(&harness.router, request).await;
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::OK, "{text}");
    let disposition = headers[header::CONTENT_DISPOSITION]
        .to_str()
        .expect("header");
    assert!(disposition.contains(".crawljob"), "{disposition}");
    assert_eq!(headers["x-rd-export-links"], "1");
    assert_eq!(
        text,
        format!(
            "text={ADDRESS}\npackageName=Holiday\nextractPasswords=[\"archive secret\"]\nautoStart=FALSE\n"
        )
    );
}

/// A LinkGrabber package exports the links not yet queued, with its own name.
#[tokio::test]
async fn a_linkgrabber_package_is_exported_too() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (_, packages, _) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Grabbed".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            file_names: vec![Some("grabbed.bin".to_owned())],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            urls: vec!["https://grabbed.example/file".parse().expect("URL")],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    let (status, file) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "collector_package_ids": [packages[0].id], "format": "rdlinks" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["packages"][0]["name"], "Grabbed");
    assert_eq!(
        file["packages"][0]["links"][0]["url"],
        "https://grabbed.example/file"
    );
}

/// Re-resolving drops the binding to the older version, so the next start claims the current
/// one; the transfer's confirmed bytes and validators stay, for the next attempt to keep when
/// the new resolution matches them. The choice is audited, without a passphrase or address.
#[tokio::test]
async fn a_download_bound_to_an_older_version_resolves_with_the_current_one_after_reresolve() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (package_id, download) =
        pinned_package(&harness.database, "Rebound", rd_core::DownloadState::Paused).await;
    let chunk = rd_core::ChunkId::new();
    harness
        .database
        .prepare_transfer(
            download,
            Some(1_048_576),
            Some("\"etag-1\"".to_owned()),
            None,
            vec![rd_db::PersistedChunk {
                id: chunk,
                start: 0,
                end: Some(1_048_575),
                committed: 0,
            }],
        )
        .await
        .expect("transfer");
    harness
        .database
        .checkpoint_chunk(chunk, 4_096)
        .await
        .expect("checkpoint");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "package_ids": [package_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 1, "{body}");
    assert_eq!(
        harness.database.resolver_pin(download).await.expect("pin"),
        None
    );
    let current = rd_core::ResolverPin {
        plugin_id: PLUGIN.parse().expect("plugin id"),
        version: "2.0.0".to_owned(),
    };
    let claimed = harness
        .database
        .claim_resolver_pin(download, current.clone())
        .await
        .expect("claim");
    assert_eq!(claimed, current, "the next start binds the current version");

    let transfer = harness
        .database
        .load_transfer(download)
        .await
        .expect("transfer");
    assert_eq!(transfer.etag.as_deref(), Some("\"etag-1\""));
    assert_eq!(transfer.chunks[0].committed, 4_096, "finished parts stay");
    let file = harness
        .database
        .get_download(download)
        .await
        .expect("download")
        .expect("there");
    assert_eq!(
        file.state,
        rd_core::DownloadState::Paused,
        "a paused file stays paused"
    );

    let (_, records) = get_json(
        &harness.router,
        "/api/v1/audit/records?action=plugin_version_chosen",
    )
    .await;
    let text = records.to_string();
    assert!(
        text.contains("reresolve") && text.contains(OLD_VERSION),
        "{text}"
    );
}

#[tokio::test]
async fn reresolve_leaves_finished_files_alone_and_refuses_an_unknown_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (_, finished) = pinned_package(
        &harness.database,
        "Finished",
        rd_core::DownloadState::Completed,
    )
    .await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reresolve",
        json!({ "ids": [finished, rd_core::DownloadId::new()] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 0, "{body}");
    assert_eq!(body["refusals"][0]["code"], "download.not_found", "{body}");
    assert!(
        harness
            .database
            .resolver_pin(finished)
            .await
            .expect("pin")
            .is_some()
    );
}
