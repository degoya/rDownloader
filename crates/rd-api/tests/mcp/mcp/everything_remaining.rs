//! RD-120-55: the thirteen capabilities RD-120-32 left unclassified, as tools -- at the price of
//! the route, with the id from a listing tool, and without a secret in any answer.
//!
//! Every call goes through `envelope`, which searches the whole answer for the canary the
//! installation keeps behind an account, a news server and a proxy. The subscription hit below
//! carries the canary a second way, as the `apikey` of its download address, which is how an
//! indexer really hands one out; `list_subscription_items` has to mask it.

use super::{
    ADMIN_BEARER, API_BEARER, CANARY, CONFIG_BEARER, INTAKE_BEARER, METRICS_BEARER, NOBODY,
    QUEUE_BEARER, READ_BEARER, SECRETS_BEARER, envelope, handshake, installation,
    installation_parts, ok,
};

/// Every tool RD-120-55 added, with arguments that reach its handler, and its route's price.
pub(super) fn remaining_tools() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    use serde_json::json;
    let (read, queue, config) = ("api:read", "api:queue", "api:config");
    let (admin, secrets, intake) = ("api:admin", "api:secrets", "api:intake");
    vec![
        ("list_remote_job_providers", json!({}), config),
        ("get_power_status", json!({}), admin),
        ("cancel_power_action", json!({}), admin),
        ("get_reconnect_status", json!({}), admin),
        ("get_metrics", json!({}), "api:metrics"),
        ("preview_diagnostic_bundle", json!({}), admin),
        (
            "list_plugin_executions",
            json!({ "id": "nothing-like-this" }),
            admin,
        ),
        ("get_plugin_messages", json!({ "locale": "en" }), read),
        ("list_account_hosters", json!({ "id": NOBODY }), secrets),
        ("get_automation_vocabulary", json!({}), config),
        ("list_automation_runs", json!({}), config),
        ("list_automation_versions", json!({ "id": NOBODY }), config),
        (
            "dry_run_automations",
            json!({ "trigger": "package_completed" }),
            queue,
        ),
        ("list_notification_deliveries", json!({}), config),
        ("list_notification_destinations", json!({}), config),
        (
            "test_category_regex",
            json!({ "pattern": "^a", "samples": ["abc"] }),
            config,
        ),
        ("get_subscription_review_summary", json!({}), config),
        ("list_subscription_items", json!({ "id": NOBODY }), config),
        ("list_subscription_runs", json!({ "id": NOBODY }), config),
        (
            "set_subscription_enabled",
            json!({ "id": NOBODY, "enabled": false }),
            config,
        ),
        ("poll_subscription", json!({ "id": NOBODY }), queue),
        (
            "review_subscription_item",
            json!({ "id": NOBODY, "state": "dismissed" }),
            config,
        ),
        (
            "review_pending_subscription_items",
            json!({ "id": NOBODY, "state": "dismissed" }),
            config,
        ),
        (
            "clear_subscription_history",
            json!({ "id": NOBODY }),
            config,
        ),
        ("list_stream_schedules", json!({}), config),
        (
            "create_stream_schedule",
            json!({ "definition": {} }),
            config,
        ),
        (
            "update_stream_schedule",
            json!({ "id": NOBODY, "definition": {} }),
            config,
        ),
        ("delete_stream_schedule", json!({ "id": NOBODY }), config),
        ("list_stream_runs", json!({}), config),
        // Refused by the route's own validation, so nothing is recorded from anywhere.
        (
            "record_stream_now",
            json!({ "url": "not an address" }),
            intake,
        ),
    ]
}

/// Each tool is refused by a token one step short of its route's permission, names that
/// permission, and is accepted by a token holding exactly it.
///
/// The near miss per price: `api:config` implies reading but not administration; `api:admin`
/// confers neither credentials nor the metrics scrape; `api:queue` is not intake. The others
/// are RD-120-32's.
#[tokio::test]
async fn every_remaining_tool_costs_what_its_route_costs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let tools = remaining_tools();

    let mut names: Vec<&str> = tools.iter().map(|(name, _, _)| *name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 30, "RD-120-55 added 30 tools");

    let mut sessions = Vec::new();
    for bearer in [
        READ_BEARER,
        QUEUE_BEARER,
        CONFIG_BEARER,
        SECRETS_BEARER,
        ADMIN_BEARER,
        INTAKE_BEARER,
        METRICS_BEARER,
    ] {
        sessions.push((bearer, handshake(&router, bearer).await));
    }
    let session = |bearer: &str| -> String {
        sessions
            .iter()
            .find(|(held, _)| *held == bearer)
            .expect("a session per token")
            .1
            .clone()
    };

    for (name, arguments, scope) in &tools {
        let (short, exact) = match *scope {
            "api:read" => (SECRETS_BEARER, READ_BEARER),
            "api:queue" => (CONFIG_BEARER, QUEUE_BEARER),
            "api:config" => (QUEUE_BEARER, CONFIG_BEARER),
            "api:admin" => (CONFIG_BEARER, ADMIN_BEARER),
            "api:secrets" => (ADMIN_BEARER, SECRETS_BEARER),
            "api:intake" => (QUEUE_BEARER, INTAKE_BEARER),
            "api:metrics" => (ADMIN_BEARER, METRICS_BEARER),
            other => panic!("no near miss for {other}"),
        };
        let answer = envelope(&router, short, &session(short), name, arguments).await;
        assert_eq!(
            answer["error"]["data"]["code"], "auth.scope_insufficient",
            "{name} was not refused without {scope}: {answer}"
        );
        assert_eq!(
            answer["error"]["data"]["scope"], *scope,
            "{name} named the wrong permission: {answer}"
        );

        let answer = envelope(&router, exact, &session(exact), name, arguments).await;
        assert!(
            answer["error"].is_null(),
            "{name} was refused with exactly {scope}: {answer}"
        );
        assert!(
            answer["result"].is_object(),
            "{name} did not reach its handler: {answer}"
        );
    }
}

/// The review list hands out the hit's id and never the indexer's key or the archive password.
#[tokio::test]
async fn a_subscription_hit_is_reviewed_by_listed_ids_without_its_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    let subscription = seed_indexer_hits(&router, &session, &database, &["one", "two"]).await;

    let summary = ok(
        &router,
        &session,
        "get_subscription_review_summary",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(summary["pending_total"], 2, "{summary}");
    assert_eq!(
        summary["subscriptions"][0]["subscription_id"],
        subscription.as_str(),
        "{summary}"
    );

    let page = ok(
        &router,
        &session,
        "list_subscription_items",
        serde_json::json!({ "id": subscription }),
    )
    .await;
    let rendered = page.to_string();
    assert!(!rendered.contains("rd-120-55-archive-password"), "{page}");
    assert!(
        rendered.contains("apikey="),
        "the address keeps its shape: {page}"
    );
    let items: Vec<String> = page["items"]
        .as_array()
        .expect("a page of items")
        .iter()
        .map(|row| row["id"].as_str().expect("item id").to_owned())
        .collect();
    assert_eq!(items.len(), 2, "{page}");

    ok(
        &router,
        &session,
        "review_subscription_item",
        serde_json::json!({ "id": items[0], "state": "dismissed" }),
    )
    .await;
    let rest = ok(
        &router,
        &session,
        "review_pending_subscription_items",
        serde_json::json!({ "id": subscription, "state": "dismissed" }),
    )
    .await;
    assert_eq!(rest["matched"], 1, "{rest}");
    let dismissed = ok(
        &router,
        &session,
        "list_subscription_items",
        serde_json::json!({ "id": subscription, "state": "dismissed" }),
    )
    .await;
    assert_eq!(dismissed["items"].as_array().map(Vec::len), Some(2));

    ok(
        &router,
        &session,
        "list_subscription_runs",
        serde_json::json!({ "id": subscription }),
    )
    .await;
    let switched = ok(
        &router,
        &session,
        "set_subscription_enabled",
        serde_json::json!({ "id": subscription, "enabled": true }),
    )
    .await;
    assert_eq!(switched["enabled"], true, "{switched}");
    // Straight back off: switched on, the service would poll an address that does not exist.
    let switched = ok(
        &router,
        &session,
        "set_subscription_enabled",
        serde_json::json!({ "id": subscription, "enabled": false }),
    )
    .await;
    assert_eq!(switched["enabled"], false, "{switched}");
    ok(
        &router,
        &session,
        "clear_subscription_history",
        serde_json::json!({ "id": subscription }),
    )
    .await;
    let cleared = ok(
        &router,
        &session,
        "list_subscription_items",
        serde_json::json!({ "id": subscription, "state": "all" }),
    )
    .await;
    assert_eq!(
        cleared["items"].as_array().map(Vec::len),
        Some(0),
        "{cleared}"
    );
}

/// An indexer subscription with one hit per key, seeded behind the tools; its id, as listed.
///
/// Also RD-120-57's starting point, which queues such a hit into the LinkGrabber.
pub(super) async fn seed_indexer_hits(
    router: &axum::Router,
    session: &str,
    database: &rd_db::Database,
    keys: &[&str],
) -> String {
    let created = ok(
        router,
        session,
        "create_subscription",
        serde_json::json!({ "definition": {
            "name": "Canary indexer", "url": "https://indexer.invalid/api", "kind": "indexer",
            "enabled": false, "mode": "review", "interval_seconds": 3600,
            "filters": {}, "backlog": { "mode": "from_now" }
        }}),
    )
    .await;
    let listed = ok(router, session, "list_subscriptions", serde_json::json!({})).await;
    let subscription = listed
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == created["id"])
        .expect("the new subscription is listed")["id"]
        .as_str()
        .expect("id")
        .to_owned();

    // Seeded behind the tools, the way a poll would store it: an indexer address carries its
    // key, and the row carries the archive password in clear.
    let hits = keys.iter().map(|key| rd_db::NewSubscriptionItem {
        item_key: (*key).to_owned(),
        title: format!("Release.{key}"),
        url: format!("https://indexer.invalid/api?t=get&id={key}&apikey={CANARY}")
            .parse()
            .expect("url"),
        published_at: None,
        duration_seconds: None,
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: Some("application/x-nzb".to_owned()),
        attributes: Default::default(),
        password: Some("rd-120-55-archive-password".to_owned()),
    });
    database
        .record_subscription_items(subscription.parse().expect("id"), hits.collect())
        .await
        .expect("record");
    subscription
}
