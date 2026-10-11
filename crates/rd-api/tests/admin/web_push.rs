//! Web Push for the installed app (RD-1240-13): the key a browser subscribes with stays the same,
//! a subscription is stored without ever handing its keys back, the first one makes the
//! `web_push` target and its rule, which saves without an address, and a subscription that is
//! not a browser's is refused.

use crate::common;

use axum::http::StatusCode;

const P256DH: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

fn subscription(endpoint: &str, device: &str, events: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "endpoint": endpoint,
        "keys": { "p256dh": P256DH, "auth": AUTH },
        "device_name": device,
        "events": events,
    })
}

#[tokio::test]
async fn the_key_is_made_once_and_stays() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;

    let (status, first) = common::get_json(&router, "/api/v1/notifications/web-push/key").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let key = first["public_key"].as_str().expect("public key").to_owned();
    // URL-safe base64 of a 65-byte uncompressed point.
    assert_eq!(key.len(), 87, "{key}");
    assert!(key.starts_with('B'), "{key}");
    let (_, again) = common::get_json(&router, "/api/v1/notifications/web-push/key").await;
    assert_eq!(again["public_key"], key.as_str());
}

#[tokio::test]
async fn a_browser_subscribes_updates_and_unsubscribes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let endpoint = "https://push.example.org/wpush/v2/abc";

    let (status, created) = common::post_json(
        &router,
        "/api/v1/notifications/web-push/subscriptions",
        subscription(endpoint, "Phone", serde_json::json!([])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["endpoint"], endpoint);
    assert_eq!(created["device_name"], "Phone");
    assert!(
        created.get("p256dh").is_none() && created.get("auth").is_none(),
        "{created}"
    );
    let id = created["id"].as_str().expect("id").to_owned();

    // The same browser again, with a choice of events: the same row.
    let (status, updated) = common::post_json(
        &router,
        "/api/v1/notifications/web-push/subscriptions",
        subscription(endpoint, "", serde_json::json!(["package_failed"])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{updated}");
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["device_name"], "Browser");
    assert_eq!(updated["events"], serde_json::json!(["package_failed"]));

    let (status, listed) =
        common::get_json(&router, "/api/v1/notifications/web-push/subscriptions").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed = listed.as_array().expect("list");
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].to_string().contains(AUTH), "{}", listed[0]);

    let (status, deleted) = common::delete_json(
        &router,
        &format!("/api/v1/notifications/web-push/subscriptions/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["code"], "notification.push_subscription_deleted");
    let (status, missing) = common::delete_json(
        &router,
        &format!("/api/v1/notifications/web-push/subscriptions/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["code"], "notification.push_subscription_not_found");
}

/// Turning push on in a browser is enough: the first subscription makes the destination and a
/// rule for every event, once. Its test action says plainly when no browser is left.
#[tokio::test]
async fn the_first_subscription_makes_the_destination() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    for endpoint in ["https://push.example.org/a", "https://push.example.org/b"] {
        let (status, body) = common::post_json(
            &router,
            "/api/v1/notifications/web-push/subscriptions",
            subscription(endpoint, "Browser", serde_json::json!([])),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (_, targets) = common::get_json(&router, "/api/v1/notifications/targets").await;
    let push: Vec<_> = targets
        .as_array()
        .expect("targets")
        .iter()
        .filter(|target| target["kind"] == "web_push")
        .collect();
    assert_eq!(push.len(), 1, "{targets}");
    let target_id = push[0]["id"].as_str().expect("id").to_owned();
    let (_, rules) = common::get_json(&router, "/api/v1/notifications/rules").await;
    let rules: Vec<_> = rules
        .as_array()
        .expect("rules")
        .iter()
        .filter(|rule| rule["target_id"] == target_id.as_str())
        .collect();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["events"], serde_json::json!([]));
    // It has no address, and saving it needs none.
    let (status, renamed) = common::put_json(
        &router,
        &format!("/api/v1/notifications/targets/{target_id}"),
        serde_json::json!({ "name": "Phones", "kind": "web_push", "endpoint": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Phones");

    let (_, listed) =
        common::get_json(&router, "/api/v1/notifications/web-push/subscriptions").await;
    for entry in listed.as_array().expect("list") {
        let id = entry["id"].as_str().expect("id");
        let (status, _) = common::delete_json(
            &router,
            &format!("/api/v1/notifications/web-push/subscriptions/{id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, tested) = common::post_json(
        &router,
        &format!("/api/v1/notifications/targets/{target_id}/test"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tested}");
    assert_eq!(tested["ok"], false);
    assert!(
        tested["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("no browser")),
        "{tested}"
    );
}

#[tokio::test]
async fn a_subscription_that_is_not_a_browsers_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let cases = [
        (
            subscription("http://push.example.org/a", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        (
            subscription("not an address", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        // The sending side's address rule (RD-1240-28): this machine and the person's own
        // network are no push service, refused when handed over rather than at every message.
        (
            subscription("https://127.0.0.1/push", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        (
            subscription("https://[::1]:8443/push", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        (
            subscription("https://192.168.178.20/push", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        (
            subscription("https://10.0.0.5/push", "x", serde_json::json!([])),
            "notification.push_endpoint_invalid",
        ),
        (
            serde_json::json!({
                "endpoint": "https://push.example.org/a",
                "keys": { "p256dh": P256DH, "auth": "short" },
            }),
            "notification.push_keys_invalid",
        ),
        (
            subscription(
                "https://push.example.org/a",
                &"d".repeat(101),
                serde_json::json!([]),
            ),
            "notification.push_device_invalid",
        ),
    ];
    for (body, code) in cases {
        let (status, refused) = common::post_json(
            &router,
            "/api/v1/notifications/web-push/subscriptions",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], code, "{refused}");
    }
    let (_, listed) =
        common::get_json(&router, "/api/v1/notifications/web-push/subscriptions").await;
    assert_eq!(listed, serde_json::json!([]));
}
