//! The download list as the database serves it (RD-1120-17): a page cut in SQL, many rows
//! removed in one writer transaction, and a new row announced on the event stream; the package
//! list's page cut in SQL too (RD-191-05).

use crate::common;

use axum::http::StatusCode;
use common::{get_json, parked_harness, post_json, test_harness};
use serde_json::json;

fn ids(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

/// Reads `uri` and answers its rows and its `X-Total-Count`, when it carries one.
async fn page(harness: &common::Harness, uri: &str) -> (serde_json::Value, Option<String>) {
    let (status, headers, bytes) = common::send_raw(
        &harness.router,
        common::request_to("GET", uri)
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    let total = headers
        .get("x-total-count")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    (serde_json::from_slice(&bytes).expect("json"), total)
}

/// A paused row of its own package, created over the REST route.
async fn create(harness: &common::Harness, url: &str, priority: &str) -> String {
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": url, "priority": priority, "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{url}: {created}");
    created["id"].as_str().expect("download id").to_owned()
}

/// Pages walked to the end give back the whole list in its own order, every page counts the
/// whole list, the last page is short and a page past the end is empty.
#[tokio::test]
async fn pages_cut_in_the_database_add_up_to_the_unpaged_list() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    // Priorities out of creation order, so the queue order is not the order of the rows.
    for (index, priority) in ["low", "high", "normal", "high", "low", "normal", "normal"]
        .into_iter()
        .enumerate()
    {
        create(
            &harness,
            &format!("https://files.example.com/paging/{index}.bin"),
            priority,
        )
        .await;
    }
    let (whole, total) = page(&harness, "/api/v1/downloads").await;
    let whole = ids(&whole);
    assert_eq!(whole.len(), 7);
    assert_eq!(total, None, "the unpaged list carries no count");

    let mut walked = Vec::new();
    for offset in [0, 3, 6] {
        let (rows, total) = page(
            &harness,
            &format!("/api/v1/downloads?limit=3&offset={offset}"),
        )
        .await;
        assert_eq!(total.as_deref(), Some("7"), "offset {offset}");
        walked.extend(ids(&rows));
    }
    assert_eq!(walked, whole, "the pages are the list, in its order");

    let (last, _) = page(&harness, "/api/v1/downloads?limit=3&offset=6").await;
    assert_eq!(ids(&last), whole[6..], "the last page is short");
    for offset in [7, 99] {
        let (beyond, total) = page(
            &harness,
            &format!("/api/v1/downloads?limit=3&offset={offset}"),
        )
        .await;
        assert!(ids(&beyond).is_empty(), "offset {offset}");
        assert_eq!(total.as_deref(), Some("7"), "offset {offset}");
    }
    let (rest, total) = page(&harness, "/api/v1/downloads?offset=2").await;
    assert_eq!(
        ids(&rest),
        whole[2..],
        "an offset alone is the rest of the list"
    );
    assert_eq!(total.as_deref(), Some("7"));
}

/// The package list pages in SQL the same way (RD-191-05): walked to the end the pages are the
/// unpaged list in its queue order, each counts all of it, and past the end a page is empty.
#[tokio::test]
async fn package_pages_cut_in_the_database_add_up_to_the_unpaged_list() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    // One package per download; priorities out of creation order, as above.
    for (index, priority) in ["normal", "high", "low", "high", "normal"]
        .into_iter()
        .enumerate()
    {
        create(
            &harness,
            &format!("https://files.example.com/package-paging/{index}.bin"),
            priority,
        )
        .await;
    }
    let (whole, total) = page(&harness, "/api/v1/packages").await;
    let whole = ids(&whole);
    assert_eq!(whole.len(), 5);
    assert_eq!(total, None, "the unpaged list carries no count");

    let mut walked = Vec::new();
    for offset in [0, 2, 4] {
        let (rows, total) = page(
            &harness,
            &format!("/api/v1/packages?limit=2&offset={offset}"),
        )
        .await;
        assert_eq!(total.as_deref(), Some("5"), "offset {offset}");
        walked.extend(ids(&rows));
    }
    assert_eq!(walked, whole, "the pages are the list, in its order");
    let (last, _) = page(&harness, "/api/v1/packages?limit=2&offset=4").await;
    assert_eq!(ids(&last), whole[4..], "the last page is short");
    let (beyond, total) = page(&harness, "/api/v1/packages?limit=2&offset=5").await;
    assert!(ids(&beyond).is_empty());
    assert_eq!(total.as_deref(), Some("5"));
    let (rest, total) = page(&harness, "/api/v1/packages?offset=3").await;
    assert_eq!(
        ids(&rest),
        whole[3..],
        "an offset alone is the rest of the list"
    );
    assert_eq!(total.as_deref(), Some("5"));
}

fn paused_row(package_id: rd_core::PackageId, index: usize) -> rd_db::NewDownload {
    rd_db::NewDownload {
        id: rd_core::DownloadId::new(),
        package_id,
        source: format!("https://files.example.com/bulk/{index}.bin")
            .parse()
            .expect("url"),
        file_name: format!("{index}.bin"),
        total_bytes: None,
        expected_checksum: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        initial_state: rd_core::DownloadState::Paused,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        enrichment: Vec::new(),
        secret_fragment: None,
    }
}

/// One package of `count` paused rows; answers their ids.
async fn package_of(
    harness: &common::Harness,
    root: &std::path::Path,
    name: &str,
    count: usize,
) -> Vec<rd_core::DownloadId> {
    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: name.to_owned(),
            destination: root.join(name).to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let mut created = Vec::with_capacity(count);
    for index in 0..count {
        let row = harness
            .database
            .create_download(paused_row(package.id, index))
            .await
            .expect("download");
        created.push(row.id);
    }
    created
}

/// The largest batch the route takes goes in one writer transaction. Before RD-1120-17 the 500
/// removals were one transaction each, about 14 s on the server; the time is printed so the
/// job file can record it.
#[tokio::test]
async fn five_hundred_downloads_are_removed_in_one_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let rows = package_of(&harness, directory.path(), "Bulk.Removal", 500).await;

    let mut events = harness.database.subscribe();
    let started = std::time::Instant::now();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        json!({ "action": "remove", "ids": rows }),
    )
    .await;
    let elapsed = started.elapsed();
    eprintln!("RD-1120-17: 500 downloads removed in {elapsed:?}");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 500, "{body}");
    assert!(
        body["refusals"].as_array().is_some_and(Vec::is_empty),
        "{body}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(14),
        "500 removals took {elapsed:?}, no faster than one transaction per row"
    );

    // One event for the batch, so no subscriber falls behind.
    let removed = batch_events(&mut events, "removed");
    assert_eq!(removed.len(), 1, "one removal event for the batch");
    assert_eq!(removed[0].payload["count"], 500);

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert!(ids(&downloads).is_empty(), "{downloads}");
    let (_, packages) = get_json(&harness.router, "/api/v1/packages").await;
    assert!(
        ids(&packages).is_empty(),
        "the package went with its last file: {packages}"
    );
}

/// A refused id is refused on its own, with the code its single removal answers, and the rest
/// of the batch is removed as before.
#[tokio::test]
async fn a_refused_id_leaves_the_rest_of_the_batch_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let rows = package_of(&harness, directory.path(), "Partly.Removed", 3).await;
    let unknown = rd_core::DownloadId::new();

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        json!({ "action": "remove", "ids": [rows[0], unknown, rows[1], rows[0]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 2, "{body}");
    let refusals = body["refusals"].as_array().expect("refusals");
    assert_eq!(
        refusals.len(),
        2,
        "the unknown id and the second mention: {body}"
    );
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal["code"] == "download.not_found"),
        "{body}"
    );

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(ids(&downloads), [rows[2].to_string()], "{downloads}");
}

/// A row created over `POST /api/v1/downloads` announces itself, so a list open elsewhere
/// shows it without a reload. Paused, so no transition follows that would announce it anyway.
#[tokio::test]
async fn a_created_download_is_announced_on_the_event_stream() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut events = harness.database.subscribe();
    const HASH: &str = "89abcdef0123456789abcdef0123456789abcdef";
    for url in [
        "https://files.example.com/announced.mkv".to_owned(),
        format!("magnet:?xt=urn:btih:{HASH}&dn=Announced.Release"),
    ] {
        let id = create(&harness, &url, "normal").await;
        let announced = batch_events(&mut events, "created");
        assert_eq!(announced.len(), 1, "{url}: one announcement per create");
        let payload = &announced[0].payload;
        assert_eq!(payload["download_id"].as_str(), Some(id.as_str()), "{url}");
        assert_eq!(payload["count"], 1, "{url}");
        assert!(
            payload.get("state").is_none(),
            "{url}: a new row is no transition: {payload}"
        );
    }
}

/// The `download.state` events waiting on `events` that carry `flag` (`created`, `removed`);
/// a receiver that fell behind fails the test, since that is what one event per row did to a
/// large batch.
fn batch_events(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
    flag: &str,
) -> Vec<rd_core::EventEnvelope> {
    let mut created = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) => {
                if event.kind == rd_core::EventKind::DownloadState && event.payload[flag] == true {
                    created.push(event);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(missed)) => {
                panic!("the subscriber fell {missed} events behind")
            }
            Err(_) => return created,
        }
    }
}

/// A LinkGrabber hand-over of 300 links is one announcement, not 300: one per row overran the
/// 512 slots of every subscriber, and each browser fell back to a resynchronisation in exactly
/// the common case of a large enqueue.
#[tokio::test]
async fn a_large_hand_over_is_announced_once_without_overrunning_the_bus() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    const LINKS: usize = 300;
    let urls: Vec<url::Url> = (0..LINKS)
        .map(|index| {
            format!("https://files.example.com/hand-over/{index}.bin")
                .parse()
                .expect("url")
        })
        .collect();
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Large.Hand.Over".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            providers: vec![None; LINKS],
            file_names: (0..LINKS)
                .map(|index| Some(format!("{index}.bin")))
                .collect(),
            sizes: vec![None; LINKS],
            requests: vec![None; LINKS],
            body_refs: vec![None; LINKS],
            urls,
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let mut events = harness.database.subscribe();

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": ids(&packages) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let announced = batch_events(&mut events, "created");
    assert_eq!(announced.len(), 1, "one announcement for the hand-over");
    let payload = &announced[0].payload;
    assert_eq!(payload["count"], LINKS, "{payload}");
    assert_eq!(
        payload["download_ids"].as_array().map(Vec::len),
        Some(100),
        "{payload}"
    );
    assert!(payload.get("state").is_none(), "{payload}");
    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(ids(&downloads).len(), LINKS);
}

/// A name read from the address is the one a person reads (RD-1240-33): `%20`, `%28` and `%29`
/// are a space and brackets in the file and in the package derived from it, a UTF-8 escape is
/// its letter, and an encoded slash stays inside the name instead of naming a folder.
#[tokio::test]
async fn a_name_read_from_the_address_is_percent_decoded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    for (url, file_name) in [
        (
            "https://files.example.com/Big%20Buck%20Test%20%282026%29.mkv",
            "Big Buck Test (2026).mkv",
        ),
        ("https://files.example.com/Caf%C3%A9.zip", "Café.zip"),
        (
            "https://files.example.com/outer%2F..%2Finner.bin",
            "outer_.._inner.bin",
        ),
    ] {
        let (status, created) = post_json(
            &harness.router,
            "/api/v1/downloads",
            json!({ "url": url, "paused": true }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{url}: {created}");
        assert_eq!(created["file_name"], file_name, "{url}");
    }
    let (_, packages) = get_json(&harness.router, "/api/v1/packages").await;
    let names: Vec<&str> = packages
        .as_array()
        .expect("packages")
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    assert!(names.contains(&"Big Buck Test (2026)"), "{names:?}");
    assert!(names.iter().all(|name| !name.contains('%')), "{names:?}");
}
