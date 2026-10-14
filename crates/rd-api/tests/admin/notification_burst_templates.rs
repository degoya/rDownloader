//! RD-1291-01: a burst of starts or of added links goes out in a rule's own words, filled once
//! when the burst closes. The owner's repro of 1.29.0 - a rule on `download_started` with its own
//! title and text delivered "Download started: sample.bin" / "Started: sample.bin." - is the first
//! test; a burst queued again after a restart still reaches each rule once, in the words it had.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use std::time::Duration;

use axum::http::StatusCode;
use rd_core::{EventEnvelope, EventKind};
use rd_notify::NotificationEvent;
use serde_json::json;

/// A burst closes ten seconds after its last occurrence.
const BURST: Duration = Duration::from_secs(40);

async fn rule(router: &axum::Router, body: serde_json::Value) -> String {
    let (status, rule) = common::post_json(router, "/api/v1/notifications/rules", body).await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    rule["id"].as_str().expect("id").to_owned()
}

async fn category(harness: &common::Harness, directory: &std::path::Path) -> rd_core::CategoryId {
    let root = harness
        .database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Primary".to_owned(),
                path: directory.join("downloads").to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    harness
        .database
        .create_category(rd_db::NewCategory {
            name: "Films".to_owned(),
            color: "#336699".to_owned(),
            storage_root_id: root.id,
            relative_path: "films".to_owned(),
            is_default: false,
            selectable: true,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            unwrap_package_folder: None,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category")
        .id
}

async fn package(
    harness: &common::Harness,
    directory: &std::path::Path,
    name: &str,
    category_id: Option<rd_core::CategoryId>,
) -> rd_core::PackageId {
    harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: name.to_owned(),
            destination: directory.join(name).to_string_lossy().into_owned(),
            category_id,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
        .id
}

/// A queued download of `package`, answered by its id.
async fn download(
    harness: &common::Harness,
    package_id: rd_core::PackageId,
    file_name: &str,
) -> rd_core::DownloadId {
    harness
        .database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: format!("https://example.com/{file_name}")
                .parse()
                .expect("url"),
            file_name: file_name.to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

/// The rules' deliveries of `event`, as (rule id, title, body).
async fn delivered(
    database: &rd_db::Database,
    event: NotificationEvent,
) -> Vec<(String, String, String)> {
    database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries")
        .into_iter()
        .filter(|delivery| delivery.event == event)
        .map(|delivery| (delivery.rule_id.to_string(), delivery.title, delivery.body))
        .collect()
}

#[tokio::test]
async fn a_burst_of_starts_goes_out_in_the_rule_s_own_words_for_its_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (_server, endpoint, _calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let films = category(&harness, directory.path()).await;
    let own = rule(
        &router,
        json!({
            "name": "films in own words",
            "target_id": target["id"],
            "events": ["download_started"],
            "category_id": films.to_string(),
            "title_template": "MYTITLE {file}",
            "body_template": "MYBODY {package} / {category} / {count}x {names}"
        }),
    )
    .await;
    let counted = rule(
        &router,
        json!({
            "name": "every start counted",
            "target_id": target["id"],
            "events": ["download_started"],
            "title_template": "{count} started [{file}]",
            "body_template": "{names}"
        }),
    )
    .await;
    let plain = rule(
        &router,
        json!({ "name": "built in", "target_id": target["id"], "events": ["download_started"] }),
    )
    .await;

    let holiday = package(&harness, directory.path(), "Holiday", Some(films)).await;
    let other = package(&harness, directory.path(), "Other", None).await;
    let mut started = vec![download(&harness, holiday, "sample.bin").await];
    for index in 0..3 {
        started.push(download(&harness, other, &format!("part{index}.rar")).await);
    }
    for id in started {
        for next in [
            rd_core::DownloadState::Resolving,
            rd_core::DownloadState::Downloading,
        ] {
            harness
                .database
                .transition_download(id, next)
                .await
                .expect("transition");
        }
    }

    let database = &harness.database;
    let mut deliveries = common::eventually(BURST, "both bursts were not queued", || async move {
        let deliveries = delivered(database, NotificationEvent::DownloadStarted).await;
        (deliveries.len() == 5).then_some(deliveries)
    })
    .await;
    deliveries.sort();
    let of = |rule: &str| -> Vec<(String, String)> {
        deliveries
            .iter()
            .filter(|(id, _, _)| id == rule)
            .map(|(_, title, body)| (title.clone(), body.clone()))
            .collect()
    };
    // The film's start alone reaches the rule bound to its category, in its words: one start
    // fills its file, package and category.
    assert_eq!(
        of(&own),
        vec![(
            "MYTITLE sample.bin".to_owned(),
            "MYBODY Holiday / Films / 1x sample.bin".to_owned()
        )],
        "{deliveries:?}"
    );
    // Several starts are counted and named, and name no one file.
    let mut words = of(&counted);
    words.sort();
    assert_eq!(
        words,
        vec![
            ("1 started [sample.bin]".to_owned(), "sample.bin".to_owned()),
            (
                "3 started []".to_owned(),
                "part0.rar, part1.rar, part2.rar".to_owned()
            ),
        ],
        "{deliveries:?}"
    );
    // A rule without a template keeps the built-in wording.
    let mut built_in: Vec<String> = of(&plain).into_iter().map(|(title, _)| title).collect();
    built_in.sort();
    assert_eq!(
        built_in,
        ["3 downloads started", "Download started: sample.bin"],
        "{deliveries:?}"
    );
}

#[tokio::test]
async fn a_burst_queued_again_after_a_restart_reaches_the_rule_once_in_the_same_words() {
    let directory = tempfile::tempdir().expect("tempdir");
    let first = common::parked_harness(directory.path()).await;
    let router = first.router.clone();
    let (_server, endpoint, _calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let own = rule(
        &router,
        json!({
            "name": "own words",
            "target_id": target["id"],
            "events": ["links_added", "download_started"],
            "title_template": "{items} via {names}",
            "body_template": "{count} imports at {time} on {date}: {message}"
        }),
    )
    .await;
    // Answered at once, so its delivery shows every earlier bus event was read.
    rule(
        &router,
        json!({ "name": "marker", "target_id": target["id"], "events": ["subscription_matched"] }),
    )
    .await;
    let marker = |database: &rd_db::Database| {
        database.broadcast(EventEnvelope::new(
            EventKind::SubscriptionChanged,
            json!({
                "resource": "subscription", "poll": "finished",
                "subscription_id": rd_core::SubscriptionId::new().to_string(),
                "found": 1, "accepted": 1, "skipped": 0, "error": null
            }),
        ));
    };
    let imports: Vec<EventEnvelope> = (0..3)
        .map(|batch| {
            EventEnvelope::new(
                EventKind::CollectorIntake,
                json!({
                    "batch_id": format!("batch-{batch}"),
                    "candidate_count": 4,
                    "package_count": 1,
                    "source": if batch == 1 { "api" } else { "clipboard" }
                }),
            )
        })
        .collect();
    for import in &imports {
        first.database.broadcast(import.clone());
    }
    marker(&first.database);
    let database = &first.database;
    common::eventually(BURST, "the marker was not queued", || async move {
        (delivered(database, NotificationEvent::SubscriptionMatched)
            .await
            .len()
            == 1)
            .then_some(())
    })
    .await;
    // Stopping queues the open burst at once (`flush_bursts(true)`).
    first.state.notifications.shutdown();
    let worded = common::eventually(BURST, "the burst was not queued", || async move {
        let deliveries = delivered(database, NotificationEvent::LinksAdded).await;
        (!deliveries.is_empty()).then_some(deliveries)
    })
    .await;
    assert_eq!(worded.len(), 1, "{worded:?}");
    let (rule_id, title, body) = worded[0].clone();
    assert_eq!(rule_id, own);
    assert_eq!(title, "12 via clipboard, API");
    let at = imports[0].occurred_at.with_timezone(&chrono::Local);
    assert_eq!(
        body,
        format!(
            "3 imports at {} on {}: 12 links arrived in the LinkGrabber from: clipboard, API.",
            at.format("%H:%M"),
            at.format("%Y-%m-%d")
        )
    );

    // The restart reads the same bus events again; a start after them is queued after them
    // when this run stops, so its delivery shows the replay went through.
    let second = common::parked_harness(directory.path()).await;
    for import in &imports {
        second.database.broadcast(import.clone());
    }
    let late = package(&second, directory.path(), "After", None).await;
    let after = download(&second, late, "after.bin").await;
    second.database.broadcast(EventEnvelope::new(
        EventKind::DownloadState,
        json!({ "download_id": after.to_string(), "previous": "queued", "state": "downloading" }),
    ));
    marker(&second.database);
    let database = &second.database;
    common::eventually(BURST, "the second marker was not queued", || async move {
        (delivered(database, NotificationEvent::SubscriptionMatched)
            .await
            .len()
            == 2)
            .then_some(())
    })
    .await;
    second.state.notifications.shutdown();
    let start = common::eventually(BURST, "the start was not queued", || async move {
        let deliveries = delivered(database, NotificationEvent::DownloadStarted).await;
        (!deliveries.is_empty()).then_some(deliveries)
    })
    .await;
    assert_eq!(start[0].1, "1 via after.bin", "{start:?}");
    assert_eq!(
        delivered(&second.database, NotificationEvent::LinksAdded).await,
        worded,
        "the replayed burst reached the rule once, in the words it had"
    );
}
