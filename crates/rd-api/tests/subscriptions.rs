//! RD-080-07: the subscription REST surface.
//!
//! The archive and the filters are unit-tested where they live; what is checked here is the
//! contract a client sees — that a subscription round-trips, that the settings that would
//! silently do nothing are refused rather than clamped, that review is the default, and that
//! the whole thing survives a restart.

mod common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_harness, test_router};
use serde_json::{Value, json};

fn body(name: &str) -> Value {
    json!({
        "name": name,
        "url": "https://example.test/c/channel",
        "kind": "media",
        "enabled": true,
        "mode": "review",
        "interval_seconds": 3_600,
        "filters": {},
        "backlog": { "mode": "from_now" }
    })
}

async fn create(router: &axum::Router, name: &str) -> Value {
    let (status, created) = post_json(router, "/api/v1/subscriptions", body(name)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created
}

#[tokio::test]
async fn a_subscription_round_trips_and_starts_in_review_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;

    assert_eq!(created["name"], "Channel");
    assert_eq!(created["kind"], "media");
    // Review by default: a subscription that starts queueing on its own is hard to undo.
    assert_eq!(created["mode"], "review");
    // Unprimed, so the backlog policy still applies to the first poll.
    assert_eq!(created["primed"], false);
    // Due immediately, so the first decision is visible without waiting an interval.
    assert!(created["next_run_at"].is_null());

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert_eq!(listed.as_array().expect("array").len(), 1);
}

#[tokio::test]
async fn an_interval_outside_the_permitted_range_is_refused_rather_than_clamped() {
    // Silently giving somebody an interval twenty times slower than they asked for is worse
    // than telling them the limit.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Too eager");
    request["interval_seconds"] = json!(30);

    let (status, response) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.interval_invalid");
    assert!(response["params"]["minimum"].is_string());
}

#[tokio::test]
async fn a_filter_range_that_can_match_nothing_is_refused() {
    // A subscription that silently accepts nothing is exactly the failure this feature is
    // meant to make visible, so it must not be creatable in the first place.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Impossible");
    request["filters"] = json!({ "min_duration_seconds": 3_600, "max_duration_seconds": 60 });

    let (status, response) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.duration_range_invalid");
}

#[tokio::test]
async fn a_non_http_address_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Local");
    request["url"] = json!("file:///etc/passwd");

    let (status, response) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "subscription.url_scheme");
}

#[tokio::test]
async fn filters_are_trimmed_and_empty_patterns_dropped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Tidy");
    request["filters"] = json!({ "title_contains": ["  review  ", "", "   "] });

    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["filters"]["title_contains"], json!(["review"]));
}

#[tokio::test]
async fn a_watched_release_page_is_capped_at_half_an_hour() {
    // RD-110-21. The floor is the kind's: a board page is not an indexer, and five minutes
    // between two requests to somebody's forum reads like a crawler. Refused rather than
    // clamped, for the reason the interval above is.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Watched board");
    request["kind"] = json!("site_rule");
    request["url"] = json!("https://board.test/tv/the-expanse/");
    request["interval_seconds"] = json!(600);

    let (status, response) = post_json(&router, "/api/v1/subscriptions", request.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.interval_invalid");
    assert_eq!(response["params"]["minimum"], "1800");

    // The same ten minutes is perfectly ordinary for every other kind.
    let mut feed = request.clone();
    feed["kind"] = json!("feed");
    feed["name"] = json!("Feed");
    let (status, created) = post_json(&router, "/api/v1/subscriptions", feed).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    request["interval_seconds"] = json!(1_800);
    request["every_release"] = json!(true);
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["kind"], "site_rule");
    assert_eq!(created["interval_seconds"], 1_800);
    // The explicit counter-choice round-trips; without it a second release of an episode is
    // recognised as the episode already had.
    assert_eq!(created["every_release"], true);
}

#[tokio::test]
async fn keeping_every_release_is_off_unless_it_is_asked_for() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    assert_eq!(created["every_release"], false);
}

#[tokio::test]
async fn a_subscription_can_be_disabled_and_enabled_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    let id = created["id"].as_str().expect("id");

    let (status, disabled) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/disable"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["enabled"], false);

    let (status, enabled) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/enable"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["enabled"], true);
}

#[tokio::test]
async fn an_edit_keeps_the_id_and_applies_the_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    let id = created["id"].as_str().expect("id").to_owned();

    let mut request = body("Renamed");
    request["mode"] = json!("auto_queue");
    let (status, updated) =
        put_json(&router, &format!("/api/v1/subscriptions/{id}"), request).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["mode"], "auto_queue");
}

#[tokio::test]
async fn deleting_one_removes_it_and_its_items_and_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    let id = created["id"].as_str().expect("id");

    let (status, _) = delete_json(&router, &format!("/api/v1/subscriptions/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert!(listed.as_array().expect("array").is_empty());
    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    assert!(items.as_array().expect("array").is_empty());
}

#[tokio::test]
async fn an_unknown_subscription_reports_a_coded_404() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let missing = rd_core::SubscriptionId::new();

    let (status, response) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{missing}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["code"], "subscription.not_found");
}

#[tokio::test]
async fn subscriptions_survive_a_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let id;
    {
        let router = test_router(temp.path()).await;
        let created = create(&router, "Channel").await;
        id = created["id"].as_str().expect("id").to_owned();
    }

    let router = test_router(temp.path()).await;
    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    let stored = listed.as_array().expect("array");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["id"], id.as_str());
    assert_eq!(stored[0]["name"], "Channel");
}

/// New persisted fields survive a restart, including the one that is never serialized.
///
/// The details are read back from the database on every request rather than held anywhere, so
/// what this actually guards is the migration and the row mapping: a column added to a table
/// that already has rows, and a stored JSON blob that has to parse back into the same map.
#[tokio::test]
async fn item_details_and_the_password_survive_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let subscription_id;
    {
        let harness = test_harness(directory.path()).await;
        let mut request = body("Indexer");
        request["kind"] = json!("indexer");
        request["api_key"] = json!("super-secret-key");
        let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        subscription_id = created["id"]
            .as_str()
            .expect("id")
            .parse::<rd_core::SubscriptionId>()
            .expect("subscription id");

        harness
            .database
            .record_subscription_items(
                subscription_id,
                vec![rd_db::NewSubscriptionItem {
                    item_key: "hit-1".to_owned(),
                    title: "Some.Movie.2024.1080p".to_owned(),
                    url: "https://indexer.test/getnzb/abc.nzb".parse().expect("url"),
                    published_at: None,
                    duration_seconds: None,
                    state: rd_core::SubscriptionItemState::Pending,
                    reason: None,
                    source_category: None,
                    media_type: Some("application/x-nzb".to_owned()),
                    attributes: [("imdbscore".to_owned(), "7.8".to_owned())]
                        .into_iter()
                        .collect(),
                    password: Some("hunter2".to_owned()),
                }],
            )
            .await
            .expect("record");
    }

    let harness = test_harness(directory.path()).await;
    let (status, items) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{subscription_id}/items"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{items}");
    assert_eq!(
        items.as_array().expect("array")[0]["attributes"]["imdbscore"],
        "7.8"
    );

    assert_eq!(items.as_array().expect("array")[0]["password"], "hunter2");
    // It also has to remain in persistence or queueing after a later restart would unpack
    // without it.
    let stored = harness
        .database
        .subscription_items(subscription_id, 10)
        .await
        .expect("items");
    assert_eq!(stored[0].password.as_deref(), Some("hunter2"));
}

#[tokio::test]
async fn an_api_key_is_never_returned() {
    // The vault reference is not a field on the wire; only whether a key exists is.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("Indexer");
    request["kind"] = json!("indexer");
    request["api_key"] = json!("super-secret-key");

    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["has_secret"], true);
    let serialized = created.to_string();
    assert!(!serialized.contains("super-secret-key"), "{serialized}");
    assert!(!serialized.contains("vault://"), "{serialized}");
}

/// What an indexer said about a hit, including its release password, reaches the client.
///
/// The attribute map comes off the wire from a third party and is served straight back, so
/// the two halves are asserted together: the details a client needs to judge a release, and
/// the archive password needed to diagnose extraction and retry it by hand.
#[tokio::test]
async fn item_details_and_the_archive_password_are_served() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let mut request = body("Indexer");
    request["kind"] = json!("indexer");
    request["api_key"] = json!("super-secret-key");
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let subscription_id: rd_core::SubscriptionId = created["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("subscription id");

    harness
        .database
        .record_subscription_items(
            subscription_id,
            vec![rd_db::NewSubscriptionItem {
                item_key: "hit-1".to_owned(),
                title: "Some.Movie.2024.1080p".to_owned(),
                url: "https://indexer.test/getnzb/abc.nzb".parse().expect("url"),
                published_at: None,
                duration_seconds: None,
                state: rd_core::SubscriptionItemState::Pending,
                reason: None,
                source_category: None,
                media_type: Some("application/x-nzb".to_owned()),
                attributes: [
                    (
                        "coverurl".to_owned(),
                        "https://indexer.test/c.jpg".to_owned(),
                    ),
                    ("imdbscore".to_owned(), "7.8".to_owned()),
                    ("size".to_owned(), "4509715660".to_owned()),
                ]
                .into_iter()
                .collect(),
                password: Some("hunter2".to_owned()),
            }],
        )
        .await
        .expect("record");

    let (status, items) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{subscription_id}/items"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{items}");
    let item = &items.as_array().expect("array")[0];

    assert_eq!(item["attributes"]["coverurl"], "https://indexer.test/c.jpg");
    assert_eq!(item["attributes"]["imdbscore"], "7.8");
    assert_eq!(item["attributes"]["size"], "4509715660");

    assert_eq!(item["password"], "hunter2");
}

#[tokio::test]
async fn item_pages_bulk_review_and_history_cleanup_cover_the_whole_archive() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut request = body("Indexer");
    request["kind"] = json!("indexer");
    request["api_key"] = json!("super-secret-key");
    let (_, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    let id: rd_core::SubscriptionId = created["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("subscription id");
    let items = (0..205)
        .map(|number| rd_db::NewSubscriptionItem {
            item_key: format!("hit-{number:03}"),
            title: format!("Release {number:03}"),
            url: format!("https://indexer.test/get/{number}.nzb")
                .parse()
                .expect("url"),
            published_at: None,
            duration_seconds: None,
            state: rd_core::SubscriptionItemState::Pending,
            reason: None,
            source_category: None,
            media_type: Some("application/x-nzb".to_owned()),
            attributes: std::collections::BTreeMap::new(),
            password: None,
        })
        .collect();
    harness
        .database
        .record_subscription_items(id, items)
        .await
        .expect("items");

    let (status, last_page) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=pending&limit=50&offset=200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{last_page}");
    assert_eq!(last_page["items"].as_array().map(Vec::len), Some(5));
    assert_eq!(last_page["total"], 205);
    assert_eq!(last_page["counts"]["pending"], 205);
    assert_eq!(last_page["run_total"], 0);

    let (_, summary) = get_json(&harness.router, "/api/v1/subscriptions/review-summary").await;
    assert_eq!(summary["pending_total"], 205);
    assert_eq!(
        summary["subscriptions"][0]["subscription_id"],
        id.to_string()
    );

    let (status, bulk) = put_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "dismissed" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bulk}");
    assert_eq!(bulk, json!({ "matched": 205, "updated": 205, "failed": 0 }));

    harness
        .database
        .record_subscription_items(
            id,
            vec![rd_db::NewSubscriptionItem {
                item_key: "later".to_owned(),
                title: "Later release".to_owned(),
                url: "https://indexer.test/get/later.nzb".parse().expect("url"),
                published_at: None,
                duration_seconds: None,
                state: rd_core::SubscriptionItemState::Pending,
                reason: None,
                source_category: None,
                media_type: Some("application/x-nzb".to_owned()),
                attributes: std::collections::BTreeMap::new(),
                password: None,
            }],
        )
        .await
        .expect("later item");
    for _ in 0..2 {
        harness
            .database
            .finish_subscription_run(
                id,
                chrono::Utc::now(),
                rd_db::PollResult {
                    found: 1,
                    accepted: 1,
                    skipped: 0,
                    error: None,
                    next_run_at: chrono::Utc::now() + chrono::Duration::hours(1),
                    consecutive_failures: 0,
                    etag: None,
                    last_modified: None,
                },
            )
            .await
            .expect("run");
    }

    let (status, removed) = delete_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/history"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed, json!({ "deleted_items": 205, "deleted_runs": 2 }));
    let (_, remaining) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=all"),
    )
    .await;
    assert_eq!(remaining["total"], 1);
    assert_eq!(remaining["items"][0]["item_key"], "later");
}

#[tokio::test]
async fn bulk_review_rejects_a_state_that_is_not_a_decision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    let id = created["id"].as_str().expect("id");

    let (status, response) = put_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "skipped" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.bulk_state_invalid");
}

#[tokio::test]
async fn bulk_queue_reports_partial_failures_and_leaves_them_pending() {
    let directory = tempfile::tempdir().expect("tempdir");
    let blocked = directory.path().join("excluded.txt");
    std::fs::write(&blocked, "blocked.test\n").expect("blocklist");
    let harness = test_harness(directory.path()).await;
    harness
        .database
        .set_setting(
            "service.settings".to_owned(),
            json!({ "excluded_domains_file": blocked }),
        )
        .await
        .expect("settings");
    let created = create(&harness.router, "Review").await;
    let id: rd_core::SubscriptionId = created["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("subscription id");
    let item = |key: &str, host: &str| rd_db::NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Release {key}"),
        url: format!("https://{host}/{key}.bin").parse().expect("url"),
        published_at: None,
        duration_seconds: None,
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: None,
    };
    harness
        .database
        .record_subscription_items(
            id,
            vec![
                item("allowed", "allowed.test"),
                item("blocked", "blocked.test"),
            ],
        )
        .await
        .expect("items");

    let (status, result) = put_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "queued" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result, json!({ "matched": 2, "updated": 1, "failed": 1 }));
    let (_, page) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=pending"),
    )
    .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["item_key"], "blocked");
}

#[tokio::test]
async fn two_auto_queued_subscription_hits_keep_distinct_passwords_to_download_packages() {
    let app = axum::Router::new().route(
        "/{name}",
        axum::routing::get(
            |axum::extract::Path(name): axum::extract::Path<String>| async move {
                let document = format!(
                    r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
                  <file poster="tester" subject="{name}.bin">
                    <groups><group>alt.binaries.test</group></groups>
                    <segments><segment bytes="42" number="1">{name}@example</segment></segments>
                  </file>
                </nzb>"#
                );
                ([("content-type", "application/x-nzb")], document)
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let mut request = body("Password Indexer");
    request["mode"] = json!("auto_queue");
    let (status, _) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED);

    let (batch, collector_packages, _) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Subscription,
            source_label: Some("Password Indexer".to_owned()),
            package_name: None,
            password: Some("incorrect-shared-value".to_owned()),
            passwords: vec![
                Some("alpha-secret".to_owned()),
                Some("beta-secret".to_owned()),
            ],
            category_id: None,
            priority: None,
            providers: vec![Some(rd_core::NZB_PROVIDER.to_owned()); 2],
            file_names: vec![
                Some("Alpha release".to_owned()),
                Some("Beta release".to_owned()),
            ],
            sizes: vec![None, None],
            requests: vec![None, None],
            body_refs: vec![None, None],
            urls: vec![
                format!("http://{address}/alpha.nzb").parse().expect("url"),
                format!("http://{address}/beta.nzb").parse().expect("url"),
            ],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(collector_packages.len(), 2);
    assert_eq!(
        collector_packages
            .iter()
            .find(|package| package.name == "Alpha release")
            .and_then(|package| package.password.as_deref()),
        Some("alpha-secret")
    );
    assert_eq!(
        collector_packages
            .iter()
            .find(|package| package.name == "Beta release")
            .and_then(|package| package.password.as_deref()),
        Some("beta-secret")
    );

    harness.link_check.check_batch(batch.id).await;
    let packages = await_package_count(&harness.router, 2).await;
    let packages = packages.as_array().expect("packages");
    let package_password = |name: &str| {
        packages
            .iter()
            .find(|package| package["name"] == name)
            .and_then(|package| package["password"].as_str())
    };
    assert_eq!(package_password("Alpha release"), Some("alpha-secret"));
    assert_eq!(package_password("Beta release"), Some("beta-secret"));
}

/// An AutoQueue subscription used to report "queued" while nothing ever downloaded: its links
/// were handed to the LinkGrabber — deliberately, so routing and the online check apply — but
/// nothing promoted them afterwards, so they sat there for good. Completing the check of a
/// batch that came from such a subscription must now enqueue its packages.
#[tokio::test]
async fn an_auto_queue_subscription_promotes_its_batch_into_the_download_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    let mut request = body("Indexer");
    request["mode"] = json!("auto_queue");
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let batch = submit_batch(
        &harness,
        rd_core::IngressSource::Subscription,
        Some("Indexer"),
    )
    .await;
    harness.link_check.check_batch(batch).await;

    let packages = await_packages(&harness.router).await;
    assert_eq!(
        packages.as_array().map(Vec::len),
        Some(1),
        "the subscription's package must reach the download queue: {packages}"
    );
}

/// RD-107-02. An auto-queue subscription must not lose what an enricher found.
///
/// The defect this covers: `enrichment_json` hung on `link_candidates` alone, and a candidate
/// an auto-queue subscription submitted is promoted within seconds. The rating was therefore
/// visible for about that long and absent everywhere it would later be looked for — the
/// download list above all.
#[tokio::test]
async fn an_auto_queued_hit_keeps_its_enrichment_on_the_package_and_the_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    let mut request = body("Indexer");
    request["mode"] = json!("auto_queue");
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let batch = submit_batch(
        &harness,
        rd_core::IngressSource::Subscription,
        Some("Indexer"),
    )
    .await;
    let candidate = harness
        .database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.batch_id == batch)
        .expect("candidate");
    harness
        .database
        .set_candidate_enrichment(
            candidate.id,
            vec![rd_core::EnrichmentField {
                name: "imdb.score".to_owned(),
                value: "9.3".to_owned(),
                plugin_id: "imdb-enricher".to_owned(),
                fetched_at: chrono::Utc::now(),
            }],
        )
        .await
        .expect("enrichment");

    harness.link_check.check_batch(batch).await;
    let packages = await_packages(&harness.router).await;
    let package = packages
        .as_array()
        .and_then(|list| list.first())
        .expect("package");
    assert_eq!(
        package["enrichment"][0]["name"], "imdb.score",
        "the package must keep the field: {packages}"
    );
    assert_eq!(package["enrichment"][0]["plugin_id"], "imdb-enricher");

    // Waited for rather than assumed: the package row and the queue rows are separate writes,
    // so reading the download list straight after the package appears is a coin toss under
    // load (RD-108-15).
    let downloads = await_downloads(&harness.router).await;
    let download = downloads
        .as_array()
        .and_then(|list| list.first())
        .expect("download");
    assert_eq!(
        download["enrichment"][0]["value"], "9.3",
        "the queue row must keep the field too: {downloads}"
    );
}

/// RD-108-15. The same fields, written in the order that used to lose.
///
/// Storing the enricher's answer and promoting the batch are two writer commands with no order
/// between them: the watcher promotes on a task of its own, so an enricher that answers while
/// the promotion is already running writes to a candidate row the enqueue has claimed — and
/// detaches moments later. Nothing here waits for a scheduler to lose the race: the whole
/// promotion is awaited first, down to the queue row, and only then does the field arrive.
/// It must end up in the same two places as in the test above.
#[tokio::test]
async fn an_enrichment_that_arrives_after_the_promotion_still_reaches_package_and_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    let mut request = body("Indexer");
    request["mode"] = json!("auto_queue");
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let batch = submit_batch(
        &harness,
        rd_core::IngressSource::Subscription,
        Some("Indexer"),
    )
    .await;
    let candidate = harness
        .database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.batch_id == batch)
        .expect("candidate");

    // The promotion runs to the end first: package row, queue row, claim released.
    harness.link_check.check_batch(batch).await;
    await_packages(&harness.router).await;
    await_downloads(&harness.router).await;

    harness
        .database
        .set_candidate_enrichment(
            candidate.id,
            vec![rd_core::EnrichmentField {
                name: "imdb.score".to_owned(),
                value: "9.3".to_owned(),
                plugin_id: "imdb-enricher".to_owned(),
                fetched_at: chrono::Utc::now(),
            }],
        )
        .await
        .expect("enrichment");

    let (_, packages) = get_json(&harness.router, "/api/v1/packages").await;
    let package = packages
        .as_array()
        .and_then(|list| list.first())
        .expect("package");
    assert_eq!(
        package["enrichment"][0]["name"], "imdb.score",
        "a late field must still reach the package: {packages}"
    );
    assert_eq!(package["enrichment"][0]["plugin_id"], "imdb-enricher");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let download = downloads
        .as_array()
        .and_then(|list| list.first())
        .expect("download");
    assert_eq!(
        download["enrichment"][0]["value"], "9.3",
        "and the queue row: {downloads}"
    );
}

/// The counterpart: only a batch a subscription submitted is promoted. A pasted batch stays in
/// the LinkGrabber for review, which is the whole point of the review list.
#[tokio::test]
async fn a_manually_pasted_batch_is_never_promoted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    let batch = submit_batch(&harness, rd_core::IngressSource::Manual, None).await;
    harness.link_check.check_batch(batch).await;

    // Give the watcher the same room the positive test gives it before concluding "nothing".
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, packages) = get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(
        packages.as_array().map(Vec::len),
        Some(0),
        "a pasted batch must stay in the LinkGrabber: {packages}"
    );
}

/// Adds one ready-to-enqueue batch. `auto_check: false` starts its link `online`, so the check
/// below has nothing to claim and finishes at once — this test is about what happens *after* a
/// check, and must not depend on reaching the network.
async fn submit_batch(
    harness: &common::Harness,
    source: rd_core::IngressSource,
    label: Option<&str>,
) -> rd_core::BatchId {
    let (batch, _, _) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source,
            source_label: label.map(str::to_owned),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            file_names: vec![None],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            urls: vec!["https://example.test/release.bin".parse().expect("url")],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    batch.id
}

/// Waits for the enqueue the watcher performs on its own task.
async fn await_packages(router: &axum::Router) -> Value {
    await_package_count(router, 1).await
}

async fn await_package_count(router: &axum::Router, count: usize) -> Value {
    await_rows(router, "/api/v1/packages", count).await
}

/// Waits for the queue rows the promotion writes after the package row.
async fn await_downloads(router: &axum::Router) -> Value {
    await_rows(router, "/api/v1/downloads", 1).await
}

/// Polls `uri` until it lists `count` rows, and says so when it never does.
///
/// The budget is generous because it is not what is being measured: under four parallel test
/// binaries the promotion takes as long as it takes. Returning the empty list on a timeout —
/// which this used to do — turned "the watcher never ran" into a mismatch on the first field
/// somebody read, which is how a missing enrichment and a missing promotion came to look the
/// same (RD-108-15).
async fn await_rows(router: &axum::Router, uri: &str, count: usize) -> Value {
    for _ in 0..200 {
        let (_, rows) = get_json(router, uri).await;
        if rows.as_array().is_some_and(|list| list.len() >= count) {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (_, rows) = get_json(router, uri).await;
    panic!("{uri} never listed {count} rows; it has {rows}");
}

/// RD-120-37: a subscription picks how the LinkGrabber draws its hits, and whether the card
/// slider turns on its own; both default to what every subscription showed before.
#[tokio::test]
async fn the_view_defaults_to_the_list_and_travels_like_every_other_setting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    assert_eq!(created["view"], "list");
    assert_eq!(created["autoplay"], false);

    let id = created["id"].as_str().expect("id").to_owned();
    let mut request = body("Channel");
    request["view"] = json!("cards");
    request["autoplay"] = json!(true);
    let (status, updated) =
        put_json(&router, &format!("/api/v1/subscriptions/{id}"), request).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["view"], "cards");
    assert_eq!(updated["autoplay"], true);

    // Read back from the store, not merely echoed by the handler.
    let (status, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert_eq!(status, StatusCode::OK);
    let stored = listed
        .as_array()
        .expect("list")
        .iter()
        .find(|entry| entry["id"] == id.as_str())
        .expect("the subscription");
    assert_eq!(stored["view"], "cards");
    assert_eq!(stored["autoplay"], true);

    // A view nobody defined is refused rather than quietly drawn as something else.
    let mut unknown = body("Carousel");
    unknown["view"] = json!("carousel");
    let (status, _) = post_json(&router, "/api/v1/subscriptions", unknown).await;
    assert!(status.is_client_error(), "{status}");
}

/// RD-120-42: a subscription picks the shape of its cards' image area; `2:1` unless asked,
/// every one of the five is kept, and anything else is refused with a stable code.
#[tokio::test]
async fn the_card_ratio_defaults_to_two_to_one_and_refuses_what_it_does_not_know() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Covers").await;
    assert_eq!(created["card_ratio"], "2:1");
    let id = created["id"].as_str().expect("id").to_owned();

    for ratio in ["1:1", "3:2", "16:9", "4:3", "2:1"] {
        let mut request = body("Covers");
        request["view"] = json!("cards");
        request["card_ratio"] = json!(ratio);
        let (status, updated) =
            put_json(&router, &format!("/api/v1/subscriptions/{id}"), request).await;
        assert_eq!(status, StatusCode::OK, "{ratio}: {updated}");
        // Read back from the store, not merely echoed by the handler.
        let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
        let stored = listed
            .as_array()
            .expect("list")
            .iter()
            .find(|entry| entry["id"] == id.as_str())
            .expect("the subscription")
            .clone();
        assert_eq!(stored["card_ratio"], ratio);
    }

    // Refused on create and on edit, and the stored ratio stays what it was.
    for unknown in ["21:9", "2/1", "", "square"] {
        let mut request = body("Wide");
        request["card_ratio"] = json!(unknown);
        let (status, answer) = post_json(&router, "/api/v1/subscriptions", request.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{unknown}: {answer}"
        );
        assert_eq!(
            answer["code"], "subscription.card_ratio_unknown",
            "{answer}"
        );
        let (status, answer) =
            put_json(&router, &format!("/api/v1/subscriptions/{id}"), request).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{unknown}: {answer}"
        );
        assert_eq!(
            answer["code"], "subscription.card_ratio_unknown",
            "{answer}"
        );
    }
    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert_eq!(
        listed.as_array().expect("list").len(),
        1,
        "nothing refused was created"
    );
    assert_eq!(listed[0]["card_ratio"], "2:1");
}

// ---- RD-130-19: script subscriptions ----

/// Writes a script into the scripts directory `test_harness` configures.
fn write_script(directory: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let scripts = directory.join("scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    let path = scripts.join(name);
    std::fs::write(&path, body).expect("script");
    path
}

fn script_body(name: &str, script: &str) -> Value {
    json!({
        "name": name,
        "url": format!("script:{script}"),
        "kind": "script",
        "mode": "review",
        "interval_seconds": 3_600,
        "schedule": "0 6 * * *",
    })
}

/// A category, with the storage root a bare harness does not have yet.
#[cfg(unix)]
async fn script_category(router: &axum::Router, directory: &std::path::Path) -> String {
    let path = directory.join("downloads");
    std::fs::create_dir_all(&path).expect("downloads");
    let (status, root) = post_json(
        router,
        "/api/v1/storage-roots",
        json!({ "name": "Primary", "path": path.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{root}");
    let (status, created) = post_json(
        router,
        "/api/v1/categories",
        json!({
            "name": "Series", "color": "#38BDF8", "storage_root_id": root["id"],
            "relative_path": "", "is_default": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("category id").to_owned()
}

/// Asks for a run and waits until the history lists `runs` of them.
async fn run_script_now(router: &axum::Router, id: &str, runs: usize) -> Value {
    let (status, answer) = post_json(
        router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    await_rows(router, &format!("/api/v1/subscriptions/{id}/runs"), runs).await
}

/// The acceptance case on Linux: a script prints links, the subscription takes each address
/// once, first for review and then straight into the queue, always with its category.
#[cfg(unix)]
#[tokio::test]
async fn a_script_subscription_takes_each_address_once_with_its_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    write_script(
        directory.path(),
        "daily-links.sh",
        "echo '# links of the day'\n\
         echo https://example.test/abc/Show.S01E01.rar\n\
         echo https://example.test/def/Show.S01E02.rar\n\
         if [ -f \"$RD_SCRIPT_DIR/more\" ]; then echo https://example.test/ghi/Show.S01E03.rar; fi\n\
         echo 'progress: done' >&2\n",
    );
    let category = script_category(router, directory.path()).await;
    let mut request = script_body("Daily links", "daily-links.sh");
    request["category_id"] = json!(category);
    let (status, created) = post_json(router, "/api/v1/subscriptions", request.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["kind"], "script");
    assert_eq!(created["url"], "script:daily-links.sh");
    assert_eq!(created["schedule"], "0 6 * * *");
    let id = created["id"].as_str().expect("id").to_owned();

    // "Run now": both links, for review -- a script has no backlog to skip.
    let runs = run_script_now(router, &id, 1).await;
    assert_eq!(runs[0]["error"], Value::Null, "{runs}");
    assert_eq!(
        (runs[0]["found"].as_u64(), runs[0]["accepted"].as_u64()),
        (Some(2), Some(2))
    );
    let (_, items) = get_json(router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let items = items.as_array().expect("items").clone();
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(
        items.iter().all(|item| item["state"] == "pending"),
        "{items:?}"
    );

    // The same output again takes nothing: every address was seen already.
    let runs = run_script_now(router, &id, 2).await;
    assert_eq!(
        (runs[0]["found"].as_u64(), runs[0]["accepted"].as_u64()),
        (Some(2), Some(0))
    );

    // Straight into the queue from now on, and a third address appears.
    request["mode"] = json!("auto_queue");
    let (status, updated) = put_json(router, &format!("/api/v1/subscriptions/{id}"), request).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    std::fs::write(directory.path().join("scripts").join("more"), "").expect("marker");
    let runs = run_script_now(router, &id, 3).await;
    assert_eq!(runs[0]["accepted"].as_u64(), Some(1), "{runs}");
    let (_, items) = get_json(router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let third = items
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["url"] == "https://example.test/ghi/Show.S01E03.rar")
        .cloned()
        .unwrap_or_else(|| panic!("the third address was not archived: {items}"));
    assert_eq!(third["state"], "queued", "{third}");

    // Handed on with the subscription's category: a LinkGrabber package while the check
    // runs, a download package once it is promoted -- either carries it.
    let mut carried = false;
    for _ in 0..200 {
        let (_, waiting) = get_json(router, "/api/v1/collector/packages").await;
        let (_, queued) = get_json(router, "/api/v1/packages").await;
        carried = [waiting, queued].iter().any(|rows| {
            rows.as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["category_id"] == category.as_str())
            })
        });
        if carried {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(carried, "no package carries the subscription's category");
}

/// A failed run stands in the history with its reason, and archives nothing it printed.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_script_run_is_in_the_history_with_its_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    write_script(
        directory.path(),
        "broken.sh",
        "echo https://example.test/half/list.rar\necho 'login refused' >&2\nexit 3\n",
    );
    let (status, created) = post_json(
        &router,
        "/api/v1/subscriptions",
        script_body("Broken", "broken.sh"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();

    let runs = run_script_now(&router, &id, 1).await;
    let error = runs[0]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("status 3") && error.contains("login refused"),
        "{runs}"
    );
    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    assert_eq!(items.as_array().map(Vec::len), Some(0), "{items}");
    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert!(
        listed[0]["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("login refused")),
        "{listed}"
    );
}

/// The same on Windows, with a `.bat` that writes CRLF lines.
#[cfg(windows)]
#[tokio::test]
async fn a_batch_file_subscription_takes_its_links() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    write_script(
        directory.path(),
        "daily-links.bat",
        "@echo off\r\n\
         echo https://example.test/abc/Show.S01E01.rar\r\n\
         echo https://example.test/def/Show.S01E02.rar\r\n",
    );
    let (status, created) = post_json(
        &router,
        "/api/v1/subscriptions",
        script_body("Daily links", "daily-links.bat"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();

    let runs = run_script_now(&router, &id, 1).await;
    assert_eq!(runs[0]["error"], Value::Null, "{runs}");
    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let urls: Vec<&str> = items
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 2, "{items}");
    assert!(
        urls.contains(&"https://example.test/def/Show.S01E02.rar"),
        "{items}"
    );
}

/// A script subscription starts code on this machine: only the administration scope creates,
/// changes, switches or runs one, whatever the route costs otherwise.
#[tokio::test]
async fn only_the_administrator_creates_or_changes_a_script_subscription() {
    use sha2::Digest;

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    write_script(
        directory.path(),
        "daily-links.sh",
        "echo https://example.test/a.rar\n",
    );
    let config_bearer = "test-config-bearer-token";
    let queue_bearer = "test-queue-bearer-token";
    for (bearer, scope) in [
        (config_bearer, rd_core::API_CONFIG_SCOPE),
        (queue_bearer, rd_core::API_QUEUE_SCOPE),
    ] {
        harness
            .database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                scope.to_owned(),
                hex::encode(sha2::Sha256::digest(bearer.as_bytes())),
                vec![scope.to_owned()],
            )
            .await
            .expect("token");
    }
    let router = &harness.router;
    let script = script_body("Daily links", "daily-links.sh");

    let (status, refused) = common::post_with_bearer(
        router,
        "/api/v1/subscriptions",
        config_bearer,
        script.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "auth.scope_insufficient");
    assert_eq!(refused["params"]["scope"], rd_core::API_ADMIN_SCOPE);

    // The same credential still manages every other kind.
    let (status, feed) =
        common::post_with_bearer(router, "/api/v1/subscriptions", config_bearer, body("Feed"))
            .await;
    assert_eq!(status, StatusCode::CREATED, "{feed}");

    let (status, created) = common::post_with_bearer(
        router,
        "/api/v1/subscriptions",
        common::API_BEARER,
        script.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");

    // Changing one -- even into something harmless -- is the administrator's too.
    let put = |bearer: &str, payload: &Value| {
        axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/subscriptions/{id}"))
            .header(axum::http::header::HOST, "127.0.0.1:8710")
            .header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {bearer}"),
            )
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(payload.to_string()))
            .expect("request")
    };
    let (status, refused) = common::send(router, put(config_bearer, &body("Now a feed"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = common::send(router, put(config_bearer, &script)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, updated) = common::send(router, put(common::API_BEARER, &script)).await;
    assert_eq!(status, StatusCode::OK, "{updated}");

    // Running it now, or switching it on or off, is the administrator's as well: the routes
    // cost `api:queue` and `api:config`, the script costs `api:admin`.
    for (bearer, action) in [
        (queue_bearer, "poll"),
        (config_bearer, "disable"),
        (config_bearer, "enable"),
    ] {
        let uri = format!("/api/v1/subscriptions/{id}/{action}");
        let (status, refused) = common::request_with_bearer(router, "POST", &uri, bearer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{action}: {refused}");
        assert_eq!(
            refused["code"], "auth.scope_insufficient",
            "{action}: {refused}"
        );
    }
    let (_, runs) = common::get_with_bearer(
        router,
        &format!("/api/v1/subscriptions/{id}/runs"),
        common::API_BEARER,
    )
    .await;
    assert_eq!(
        runs.as_array().map(Vec::len),
        Some(0),
        "nothing ran: {runs}"
    );
    for (action, expected) in [
        ("disable", StatusCode::OK),
        ("enable", StatusCode::OK),
        ("poll", StatusCode::ACCEPTED),
    ] {
        let uri = format!("/api/v1/subscriptions/{id}/{action}");
        let (status, answer) =
            common::request_with_bearer(router, "POST", &uri, common::API_BEARER).await;
        assert_eq!(status, expected, "{action}: {answer}");
    }
    // The same routes keep their own price for every other kind.
    let feed_id = feed["id"].as_str().expect("feed id");
    let uri = format!("/api/v1/subscriptions/{feed_id}/poll");
    let (status, answer) = common::request_with_bearer(router, "POST", &uri, queue_bearer).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
}

/// What would only fail the next morning is refused when it is saved.
#[tokio::test]
async fn a_script_subscription_is_refused_for_what_would_fail_it_later() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    write_script(
        directory.path(),
        "daily-links.sh",
        "echo https://example.test/a.rar\n",
    );

    let mut cases = Vec::new();
    let mut unknown = script_body("Missing", "nowhere.sh");
    unknown["schedule"] = Value::Null;
    cases.push((unknown, "subscription.script_not_found"));
    for bad in ["../daily-links.sh", ".hidden.sh", "a b.sh", ""] {
        cases.push((script_body("Bad", bad), "subscription.script_name_invalid"));
    }
    for bad in ["every morning", "0 0 6 * * *", "0 0 30 2 *"] {
        let mut request = script_body("Bad schedule", "daily-links.sh");
        request["schedule"] = json!(bad);
        cases.push((request, "subscription.schedule_invalid"));
    }
    // Only a script runs on a schedule; a feed keeps the interval and its floor.
    let mut feed = body("Feed");
    feed["schedule"] = json!("* * * * *");
    cases.push((feed, "subscription.schedule_kind"));

    for (request, code) in cases {
        let (status, answer) = post_json(&router, "/api/v1/subscriptions", request.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{request}: {answer}"
        );
        assert_eq!(answer["code"], code, "{request}: {answer}");
    }

    // A bare name is what somebody types, and is stored as the address.
    let mut bare = script_body("Bare", "daily-links.sh");
    bare["url"] = json!("daily-links.sh");
    bare["schedule"] = json!("  ");
    let (status, created) = post_json(&router, "/api/v1/subscriptions", bare).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["url"], "script:daily-links.sh");
    assert_eq!(created["schedule"], Value::Null);
    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
}
