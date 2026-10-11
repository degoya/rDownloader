//! The automatic install of an offered update (RD-1240-27), up to the hand-over.
//!
//! When it installs is `rd_update::auto_install`'s tests, with a clock and made-up moments. What
//! is checked here is the service's side: the setting and its window, the switch that does
//! nothing where the installation does not install itself, the install through the same steps as
//! the click, the audit record, and the notices before and after. The loop's look is called with
//! a clock of the test's own instead of waiting for it.

use crate::common;
use crate::updates::{ARTIFACT_BYTES, install_reaches, installable};

use axum::http::StatusCode;
use chrono::{Duration, Timelike, Utc};
use common::{get_json, post_json, put_json};
use rd_api::update_auto_install::{AutoInstaller, Tick};
use rd_update::auto_install::Wait;
use serde_json::json;

/// Saves the settings document with `changes` over the current one.
async fn set(harness: &common::Harness, changes: serde_json::Value) -> serde_json::Value {
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    // As in `updates`: the harness runs with the login off.
    settings["admin_login_disabled"] = json!(true);
    for (key, value) in changes.as_object().expect("object") {
        settings[key] = value.clone();
    }
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// A rule that hears every update event, so the deliveries show what was announced.
async fn listen(harness: &common::Harness) {
    let (status, target) = post_json(
        &harness.router,
        "/api/v1/notifications/targets",
        json!({ "name": "hook", "kind": "webhook", "endpoint": "http://127.0.0.1:9/hook" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    let (status, rule) = post_json(
        &harness.router,
        "/api/v1/notifications/rules",
        json!({
            "name": "updates",
            "target_id": target["id"],
            "events": ["update_available", "update_installed", "update_failed"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
}

async fn delivered(harness: &common::Harness) -> Vec<(rd_notify::NotificationEvent, String)> {
    harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries")
        .into_iter()
        .map(|delivery| (delivery.event, delivery.title))
        .collect()
}

/// Off by default and off it never installs; switched on, it waits out the quiet period and then
/// installs through the same steps as the click, recorded as the service's own and announced.
#[tokio::test]
async fn switched_on_it_installs_after_the_quiet_period_and_off_never() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    listen(&harness).await;
    let (_, status) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(status["auto_install"], false, "{status}");
    assert_eq!(status["installs_itself"], true, "{status}");

    let mut installer = AutoInstaller::default();
    let now = Utc::now();
    for minutes in [0, 10, 600] {
        let tick = installer
            .tick(&harness.state, now + Duration::minutes(minutes))
            .await;
        assert_eq!(tick, Tick::Waiting(Wait::Off));
    }
    assert!(launched.lock().expect("launched").is_empty());

    set(&harness, json!({ "update_auto_install": true })).await;
    let (_, status) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(status["auto_install"], true, "{status}");
    let mut installer = AutoInstaller::default();
    assert_eq!(
        installer.tick(&harness.state, now).await,
        Tick::Waiting(Wait::Settling)
    );
    assert_eq!(
        installer
            .tick(&harness.state, now + Duration::minutes(6))
            .await,
        Tick::Started("99.0.0".to_owned())
    );
    install_reaches(&harness, "restarting").await;
    let journal = launched.lock().expect("launched")[0].clone();
    assert_eq!(journal.plan.target_version, "99.0.0");

    let started: Vec<_> = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .filter(|record| record.action == rd_core::AuditAction::UpdateInstallStarted)
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(
        started[0].details.get("automatic").map(String::as_str),
        Some("true"),
        "{started:?}"
    );
    assert_eq!(started[0].actor_kind, rd_core::AuditActorKind::System);
    let notices = delivered(&harness).await;
    assert!(
        notices.iter().any(|(event, title)| *event
            == rd_notify::NotificationEvent::UpdateAvailable
            && title.contains("Installing rDownloader 99.0.0")),
        "{notices:?}"
    );

    // An install under way is not started again; once taken back, it is announced once and the
    // version waits for a person.
    assert_eq!(
        installer
            .tick(&harness.state, now + Duration::minutes(7))
            .await,
        Tick::Waiting(Wait::Installing)
    );
    let mut ended = journal;
    ended
        .end(
            rd_update::install::Phase::RolledBack,
            "update.health_timeout",
            "the test says so",
        )
        .expect("end");
    install_reaches(&harness, "rolled_back").await;
    for minutes in [20, 30] {
        assert_eq!(
            installer
                .tick(&harness.state, now + Duration::minutes(minutes))
                .await,
            Tick::Waiting(Wait::FailedBefore)
        );
    }
    let failed = delivered(&harness)
        .await
        .into_iter()
        .filter(|(event, _)| *event == rd_notify::NotificationEvent::UpdateFailed)
        .count();
    assert_eq!(failed, 1);
    assert_eq!(launched.lock().expect("launched").len(), 1);
}

/// A package manager's installation is not installed by the switch; the status says so.
#[tokio::test]
async fn an_installation_that_does_not_install_itself_is_left_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Deb,
        ARTIFACT_BYTES,
    )
    .await;
    set(&harness, json!({ "update_auto_install": true })).await;
    let (_, status) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(status["installs_itself"], false, "{status}");
    let mut installer = AutoInstaller::default();
    let now = Utc::now();
    for minutes in [0, 10] {
        assert_eq!(
            installer
                .tick(&harness.state, now + Duration::minutes(minutes))
                .await,
            Tick::Waiting(Wait::Unsupported)
        );
    }
    assert!(launched.lock().expect("launched").is_empty());
}

/// The window holds the install until it opens; an empty one is refused.
#[tokio::test]
async fn the_install_window_is_kept_and_validated() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let now = Utc::now();
    let minute = u16::try_from(now.hour() * 60 + now.minute()).expect("minute");
    let (_, settings) = get_json(&harness.router, "/api/v1/settings").await;
    let mut empty = settings.clone();
    empty["admin_login_disabled"] = json!(true);
    empty["update_auto_install_window"] = json!({ "start_minute": 180, "end_minute": 180 });
    let (status, body) = put_json(&harness.router, "/api/v1/settings", empty).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["code"], "settings.update_auto_install_window_invalid",
        "{body}"
    );

    // Opens an hour from now, in UTC, and closes an hour later.
    let saved = set(
        &harness,
        json!({
            "update_auto_install": true,
            "bandwidth_timezone": "UTC",
            "update_auto_install_window": {
                "start_minute": (minute + 60) % 1440,
                "end_minute": (minute + 120) % 1440
            }
        }),
    )
    .await;
    assert_eq!(
        saved["update_auto_install_window"]["start_minute"],
        (minute + 60) % 1440,
        "{saved}"
    );
    let mut installer = AutoInstaller::default();
    assert_eq!(
        installer.tick(&harness.state, now).await,
        Tick::Waiting(Wait::Settling)
    );
    assert_eq!(
        installer
            .tick(&harness.state, now + Duration::minutes(6))
            .await,
        Tick::Waiting(Wait::OutsideWindow)
    );
    assert_eq!(
        installer
            .tick(&harness.state, now + Duration::minutes(61))
            .await,
        Tick::Started("99.0.0".to_owned())
    );
    install_reaches(&harness, "restarting").await;
    assert_eq!(launched.lock().expect("launched").len(), 1);
}
