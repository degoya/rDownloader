//! The language of the notifications (RD-1480-11): the built-in wording follows the setting,
//! English while none is chosen, a rule's own wording stays as it was written, and a webhook's
//! body says when it happened and to which package.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use axum::http::StatusCode;
use rd_core::{PackageId, PackageState};
use rd_notify::{NotificationLanguage as L, Text};
use serde_json::json;

/// A package that just finished, by its name; answers its id.
async fn finished(harness: &common::Harness, name: &str) -> PackageId {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": format!("https://example.test/{name}.bin"), "package_name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: PackageId = created["package_id"]
        .as_str()
        .expect("package id")
        .parse()
        .expect("package id");
    harness
        .database
        .set_package_state(id, PackageState::Completed, None, None, None)
        .await
        .expect("state");
    id
}

fn title(language: L, name: &str) -> String {
    Text::new("package_completed.title")
        .with("name", name)
        .say(language)
}

#[tokio::test]
async fn the_built_in_wording_follows_the_setting_and_an_own_one_does_not() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (_server, endpoint, calls) = spawn_receiver().await;
    // A target each, so one sweep of the queue delivers both.
    for (name, template) in [("built-in", None), ("own", Some("Finished now: {package}"))] {
        let target = create_target(
            &router,
            json!({ "name": name, "kind": "webhook", "endpoint": endpoint }),
        )
        .await;
        let (status, rule) = common::post_json(
            &router,
            "/api/v1/notifications/rules",
            json!({
                "name": name,
                "target_id": target["id"],
                "events": ["package_completed"],
                "title_template": template
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{rule}");
    }
    let bodies = || {
        calls
            .lock()
            .expect("calls")
            .iter()
            .map(|(_, _, body)| serde_json::from_str(body).expect("a JSON body"))
            .collect::<Vec<serde_json::Value>>()
    };
    let arrived = |count: usize| {
        let calls = &calls;
        common::eventually(
            std::time::Duration::from_secs(30),
            "the deliveries did not arrive",
            move || async move { (calls.lock().expect("calls").len() >= count).then_some(()) },
        )
    };

    let (status, mut settings) = common::get_json(&router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["notification_language"], "en");
    let english = finished(&harness, "Holiday").await;
    arrived(2).await;
    let body = bodies()
        .into_iter()
        .find(|body| body["title"] == title(L::En, "Holiday"))
        .expect("English while none is chosen");
    assert_eq!(body["event"], "package_completed");
    assert_eq!(body["version"], rd_notify::PAYLOAD_VERSION);
    assert_eq!(body["package_id"], english.to_string());
    assert!(body["download_id"].is_null(), "{body}");
    let occurred_at: rd_core::Timestamp = body["occurred_at"]
        .as_str()
        .expect("a moment")
        .parse()
        .expect("RFC 3339");
    assert!(occurred_at <= rd_core::Timestamp::now());

    // The harness switches the login off in memory only; the route saves the stored document, whose
    // default is on, and the next request would answer `auth.setup_pending`.
    settings["admin_login_disabled"] = json!(true);
    settings["notification_language"] = json!("de");
    let (status, saved) = common::put_json(&router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["notification_language"], "de");
    finished(&harness, "Autumn").await;
    arrived(4).await;
    let titles: Vec<_> = bodies()
        .into_iter()
        .map(|body| body["title"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_ne!(title(L::De, "Autumn"), title(L::En, "Autumn"));
    for wanted in [
        title(L::De, "Autumn"),
        "Finished now: Holiday".to_owned(),
        "Finished now: Autumn".to_owned(),
    ] {
        assert!(titles.contains(&wanted), "{wanted} in {titles:?}");
    }
}

/// The test message speaks the chosen language too, and a language nobody wrote is refused.
#[tokio::test]
async fn the_test_message_is_translated_and_an_unknown_language_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_server, endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "language test", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (_, mut settings) = common::get_json(&router, "/api/v1/settings").await;
    // The harness switches the login off in memory only; the route saves the stored document, whose
    // default is on, and the next request would answer `auth.setup_pending`.
    settings["admin_login_disabled"] = json!(true);
    settings["notification_language"] = json!("xx");
    let (status, refused) = common::put_json(&router, "/api/v1/settings", settings.clone()).await;
    assert!(status.is_client_error(), "{status}: {refused}");
    settings["notification_language"] = json!("fr");
    let (status, saved) = common::put_json(&router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let id = target["id"].as_str().expect("id");
    let (status, result) = common::post_json(
        &router,
        &format!("/api/v1/notifications/targets/{id}/test"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let body: serde_json::Value = {
        let calls = calls.lock().expect("calls");
        serde_json::from_str(&calls.first().expect("a call").2).expect("a JSON body")
    };
    assert_eq!(body["title"], Text::new("test.title").say(L::Fr));
    assert_eq!(body["body"], Text::new("test.body").say(L::Fr));
    assert_eq!(body["version"], rd_notify::PAYLOAD_VERSION);
    assert!(body["occurred_at"].is_string(), "{body}");
}
