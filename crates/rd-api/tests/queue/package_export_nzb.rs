//! A package export carries its NZBs; `.crawljob` goes in through the import dialog (RD-1220-02).
//!
//! The owner's case: a LinkGrabber package of indexer hits was exported with a passphrase,
//! deleted and imported again, and came back as the indexer's API addresses — "not resolvable",
//! because no hoster plugin takes `api.<indexer>`. The file now carries the NZB documents, never
//! the indexer's address or its key, and the import takes them in like a dropped NZB.

use crate::common;

use axum::http::{StatusCode, header};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::{delete_json, parked_harness, post_json, request_to, send_raw};
use serde_json::{Value, json};

/// The key an indexer address carries; it must never reach a file.
const KEY: &str = "SECRETKEY0123";

/// A one-file NZB an indexer would answer with.
const DOCUMENT: &str = r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="show.s01e01.mkv">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">show-part1@example.test</segment></segments>
  </file>
</nzb>"#;

/// An indexer: `/api` answers with the NZB and its release name, `/refuse` with a refusal inside
/// a `200 OK`, as an indexer at its API limit does.
async fn indexer() -> std::net::SocketAddr {
    let app = axum::Router::new()
        .route(
            "/api",
            axum::routing::get(|| async {
                (
                    [
                        ("content-type", "application/x-nzb"),
                        ("x-dnzb-name", "Show.S01E01"),
                    ],
                    DOCUMENT,
                )
            }),
        )
        .route(
            "/refuse",
            axum::routing::get(|| async {
                (
                    [
                        ("x-dnzb-rcode", "429"),
                        ("x-dnzb-rtext", "Request limit reached"),
                    ],
                    "",
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// A LinkGrabber package of two indexer hits — one delivered, one refused — and a hoster link.
async fn indexer_package(
    database: &rd_db::Database,
    address: std::net::SocketAddr,
) -> rd_core::CollectorPackageId {
    let (_, packages, _) = database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Subscription,
            source_label: None,
            package_name: Some("Indexer".to_owned()),
            password: Some("package secret".to_owned()),
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![
                Some(rd_core::NZB_PROVIDER.to_owned()),
                Some(rd_core::NZB_PROVIDER.to_owned()),
                None,
            ],
            file_names: vec![None, Some("Refused.Release".to_owned()), None],
            sizes: vec![None, None, None],
            requests: vec![None, None, None],
            body_refs: vec![None, None, None],
            urls: vec![
                format!("http://{address}/api?t=get&id=5&apikey={KEY}")
                    .parse()
                    .expect("URL"),
                format!("http://{address}/refuse?id=6&apikey={KEY}")
                    .parse()
                    .expect("URL"),
                "https://grabbed.example/file".parse().expect("URL"),
            ],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    packages.first().expect("collector package").id
}

fn import_body(file_name: &str, content: &[u8], passphrase: Option<&str>, enqueue: bool) -> Value {
    let mut body = json!({ "file_name": file_name, "content": STANDARD.encode(content) });
    if let Some(passphrase) = passphrase {
        body["passphrase"] = passphrase.into();
    }
    if enqueue {
        body["enqueue"] = "true".into();
    }
    body
}

/// The export as the browser receives it: status, headers and the file.
async fn export_raw(
    router: &axum::Router,
    request: Value,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let request = request_to("POST", "/api/v1/packages/export")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(request.to_string()))
        .expect("request");
    let (status, headers, bytes) = send_raw(router, request).await;
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn an_indexer_hit_travels_as_its_nzb_without_the_indexer_or_its_key() {
    let address = indexer().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let package = indexer_package(&harness.database, address).await;

    let (status, headers, text) = export_raw(
        &harness.router,
        json!({ "collector_package_ids": [package], "format": "rdlinks" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(headers["x-rd-export-links"], "1");
    assert_eq!(headers["x-rd-export-nzbs"], "1");
    // The refused hit is skipped, not the export, and named with the indexer's reason.
    assert_eq!(headers["x-rd-export-skipped"], "1");
    let failed = headers["x-rd-export-failed"].to_str().expect("ASCII");
    assert!(failed.contains("collector.nzb_rejected"), "{failed}");
    assert!(failed.contains("Refused.Release"), "{failed}");
    assert!(!failed.contains(KEY), "{failed}");
    assert!(
        !text.contains(KEY) && !text.contains(&address.to_string()),
        "neither the indexer nor its key is in the file: {text}"
    );
    let file: Value = serde_json::from_str(&text).expect("JSON");
    let carried = &file["packages"][0];
    assert_eq!(carried["links"][0]["url"], "https://grabbed.example/file");
    assert_eq!(carried["links"].as_array().map(Vec::len), Some(1));
    assert_eq!(carried["nzbs"][0]["name"], "Show.S01E01");
    let content = carried["nzbs"][0]["content"].as_str().expect("XML");
    assert!(content.contains("show-part1@example.test"), "{content}");
    // The password the enqueue would have taken travels inside the NZB.
    assert!(content.contains("package secret"), "{content}");
}

/// The owner's round trip, sealed: export, delete, import — the NZB lands as a dropped NZB,
/// and nothing is left as an address no plugin resolves.
#[tokio::test]
async fn a_sealed_export_brings_the_nzb_back_as_an_nzb_import() {
    let address = indexer().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let package = indexer_package(&harness.database, address).await;

    let (status, _, text) = export_raw(
        &harness.router,
        json!({ "collector_package_ids": [package], "format": "rdlinks", "passphrase": "correct horse" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        !text.contains("show-part1") && !text.contains("Show.S01E01"),
        "the NZBs are sealed with the links: {text}"
    );
    let (status, body) = delete_json(
        &harness.router,
        &format!("/api/v1/collector/packages/{package}"),
    )
    .await;
    assert!(status.is_success(), "{body}");

    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body(
            "indexer.rdlinks",
            text.as_bytes(),
            Some("correct horse"),
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    let candidates = imported["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 1, "only the hoster link: {imported}");
    assert_eq!(candidates[0]["url"], "https://grabbed.example/file");
    let nzb = &imported["nzb_imports"][0];
    assert_eq!(nzb["name"], "Show.S01E01", "{imported}");
    assert_eq!(nzb["state"], "imported", "{imported}");
    let id: rd_core::NzbImportId = serde_json::from_value(nzb["id"].clone()).expect("import id");
    let stored = harness
        .database
        .get_nzb_import(id)
        .await
        .expect("import")
        .expect("there");
    assert_eq!(stored.password.as_deref(), Some("package secret"));
    assert!(!imported.to_string().contains(KEY));
}

/// A queued Usenet download used to be skipped without a word; it goes out as its NZB and comes
/// back straight into the download list when the import asks for that.
#[tokio::test]
async fn a_usenet_download_travels_as_its_nzb_and_comes_back_queued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let import = harness
        .database
        .add_nzb_import(rd_db::NewNzbImport {
            name: "Usenet.Release.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            priority: None,
            import_mode: rd_core::ImportMode::Review,
            source: rd_core::IngressSource::Manual,
            source_path: None,
            password: Some("usenet secret".to_owned()),
            announce_arrival: false,
            files: vec![rd_db::NewNzbFile {
                subject: "\"usenet.release.mkv\" yEnc (1/1)".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![rd_db::NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "usenet-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let queued = harness
        .database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("downloads"),
            rd_core::DownloadPriority::Normal,
            true,
        )
        .await
        .expect("queued");

    let (status, file) = post_json(
        &harness.router,
        "/api/v1/packages/export",
        json!({ "package_ids": [queued.id], "format": "rdlinks" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let carried = &file["packages"][0];
    assert_eq!(carried["links"], json!([]), "{file}");
    assert_eq!(carried["nzbs"][0]["name"], "Usenet.Release");
    let content = carried["nzbs"][0]["content"].as_str().expect("XML");
    assert!(content.contains("usenet-1@example.test"), "{content}");

    let (status, body) = delete_json(
        &harness.router,
        &format!("/api/v1/packages/{}?force=true", queued.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("usenet.rdlinks", file.to_string().as_bytes(), None, true),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(
        imported["nzb_imports"][0]["state"], "enqueued",
        "{imported}"
    );
    let id: rd_core::NzbImportId =
        serde_json::from_value(imported["nzb_imports"][0]["id"].clone()).expect("import id");
    let packages = harness.database.list_packages().await.expect("packages");
    assert!(
        packages
            .iter()
            .any(|package| package.nzb_import_id == Some(id)),
        "the NZB is in the download list again"
    );
}

/// A crawljob export would carry the indexer's address; it carries nothing of the hit instead.
#[tokio::test]
async fn a_crawljob_fetches_no_nzb_and_counts_it_as_skipped() {
    let address = indexer().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let package = indexer_package(&harness.database, address).await;

    let (status, headers, text) = export_raw(
        &harness.router,
        json!({ "collector_package_ids": [package], "format": "crawljob" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(headers["x-rd-export-skipped"], "2");
    assert_eq!(headers["x-rd-export-nzbs"], "0");
    assert!(
        !text.contains(KEY) && !text.contains(&address.to_string()),
        "{text}"
    );
}

/// An edited NZB refuses the whole file, before a single link is taken in.
#[tokio::test]
async fn a_broken_embedded_nzb_refuses_the_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let file = json!({ "format": "rdownloader-links/1", "packages": [{
        "name": "Broken",
        "links": [{ "url": "https://grabbed.example/file" }],
        "nzbs": [{ "name": "Broken.Release", "content": "<not an nzb" }]
    }]});
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("broken.rdlinks", file.to_string().as_bytes(), None, false),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "rdlinks.nzb_invalid");
    assert!(
        harness
            .database
            .list_candidates()
            .await
            .expect("candidates")
            .is_empty(),
        "nothing was taken in"
    );
}

/// A `.crawljob` goes in through the dialog's container import, read with the plugin's rules:
/// links, package names and passwords, and no folder or start of its own.
#[tokio::test]
async fn a_crawljob_goes_in_through_the_container_import() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let crawljob = "text=https://ddownload.example/abc123/holiday.part1.rar https://ddownload.example/def456/holiday.part2.rar\n\
                    packageName=Holiday 2026\n\
                    extractPasswords=[\"se\\\"cret\"]\n\
                    downloadFolder=/etc\n\
                    autoStart=TRUE\n\
                    \n\
                    text=http://example.com/file.bin\n\
                    packageName=Second line\n";
    let (status, imported) = post_json(
        &harness.router,
        "/api/v1/containers/import",
        import_body("jdownloader.crawljob", crawljob.as_bytes(), None, false),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["format"], "crawljob");
    assert_eq!(imported["packages"][0]["name"], "Holiday 2026");
    assert_eq!(imported["packages"][0]["password"], "se\"cret");
    assert_eq!(imported["packages"][1]["name"], "Second line");
    assert_eq!(imported["candidates"].as_array().map(Vec::len), Some(3));
    assert!(
        harness
            .database
            .list_packages()
            .await
            .expect("packages")
            .is_empty(),
        "autoStart started nothing"
    );
}
