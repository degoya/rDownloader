//! A pending restart and the restart itself (RD-1240-32).
//!
//! What is pending comes from the plugin lifecycle and from the routes that record what it does
//! not show; a start has nothing pending. How the service comes back follows the install kind
//! and the environment, which the tests give the state instead of reading this process's; no
//! test starts a process or stops the runtime -- the relauncher is a closure here, and the
//! supervised restart only cancels the shutdown token and sets the exit code `serve` reads. The
//! automatic restart's look is called with a clock of the test's own, as the automatic install's.

use crate::common;
use crate::plugin_versions::{PLUGIN, install_package};

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use chrono::{Duration, Timelike, Utc};
use common::{
    API_BEARER, CAPTURE_BEARER, READ_BEARER, get_json, get_with_bearer, parked_harness, patch_json,
    post_json, post_with_bearer, put_json,
};
use rd_api::restart_auto::{AutoRestarter, Tick};
use rd_api_core::restart_state::Relauncher;
use rd_update::RestartPlan;
use rd_update::auto_install::Wait;
use rd_update::install::Journal;
use rd_update::restart::Environment;
use serde_json::json;

const STATUS: &str = "/api/v1/system/restart";

/// Installs as `kind`, in `environment`, with a relauncher that records its plans.
fn installed_as(
    harness: &common::Harness,
    directory: &std::path::Path,
    kind: rd_update::InstallKind,
    environment: Environment,
) -> Arc<Mutex<Vec<RestartPlan>>> {
    harness.state.updates.use_installation(
        kind,
        directory.to_path_buf(),
        Arc::new(|_: &Journal| -> anyhow::Result<()> { Ok(()) }),
    );
    let plans = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&plans);
    let relauncher: Relauncher = Arc::new(move |plan: &RestartPlan| -> anyhow::Result<()> {
        recorded.lock().expect("plans").push(plan.clone());
        Ok(())
    });
    harness
        .state
        .restart
        .use_environment(environment, relauncher);
    plans
}

fn systemd() -> Environment {
    Environment {
        systemd: true,
        relaunchable: true,
    }
}

fn by_hand() -> Environment {
    Environment {
        systemd: false,
        relaunchable: true,
    }
}

/// Switches the fixture plugin off: recorded, since the lifecycle does not show it.
async fn switch_off(harness: &common::Harness) {
    let (status, body) = patch_json(
        &harness.router,
        &format!("/api/v1/plugins/{PLUGIN}"),
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A start has nothing pending; a plugin version installed after it is pending with the version
/// it runs now and the one the next start runs; the next start's record clears it.
#[tokio::test]
async fn a_plugin_installed_after_the_start_makes_a_restart_pending_until_the_next_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    installed_as(
        &harness,
        directory.path(),
        rd_update::InstallKind::Portable,
        by_hand(),
    );
    install_package(directory.path(), "1.0.0");
    harness
        .state
        .plugins
        .record_started_versions()
        .await
        .expect("the start records what it loads");
    rd_api::restart_service::remember_baseline(&harness.state).await;
    let (status, body) = get_json(&harness.router, STATUS).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pending"], false, "{body}");
    assert_eq!(body["reasons"], json!([]), "{body}");
    assert_eq!(body["can_restart"], true, "{body}");
    assert_eq!(body["how"], "self", "{body}");
    assert!(body["supervisor"].is_null(), "{body}");
    assert_eq!(body["automatic"], false, "off by default: {body}");
    assert!(
        body["started_at"].as_str().is_some_and(|at| !at.is_empty()),
        "{body}"
    );

    install_package(directory.path(), "2.0.0");
    let (_, body) = get_json(&harness.router, STATUS).await;
    assert_eq!(body["pending"], true, "{body}");
    let reason = &body["reasons"][0];
    assert_eq!(reason["code"], "plugin_updated", "{body}");
    assert_eq!(reason["plugin_id"], PLUGIN, "{body}");
    assert_eq!(reason["name"], "Lifecycle", "{body}");
    assert_eq!(reason["from_version"], "1.0.0", "{body}");
    assert_eq!(reason["version"], "2.0.0", "{body}");

    // What the next start does: it records 2.0.0 as loaded, and nothing waits any more.
    harness
        .state
        .plugins
        .record_started_versions()
        .await
        .expect("the next start");
    let (_, body) = get_json(&harness.router, STATUS).await;
    assert_eq!(body["pending"], false, "{body}");
}

/// Switching a plugin off is recorded as pending; switching it on again replaces that reason.
#[tokio::test]
async fn a_plugin_switched_off_is_recorded_as_pending() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_package(directory.path(), "1.0.0");
    switch_off(&harness).await;
    let (_, body) = get_json(&harness.router, STATUS).await;
    assert_eq!(body["pending"], true, "{body}");
    let codes: Vec<&str> = body["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .filter_map(|reason| reason["code"].as_str())
        .collect();
    assert!(codes.contains(&"plugin_disabled"), "{body}");
    let (status, _) = patch_json(
        &harness.router,
        &format!("/api/v1/plugins/{PLUGIN}"),
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get_json(&harness.router, STATUS).await;
    let disabled = body["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .filter(|reason| reason["code"] == "plugin_disabled")
        .count();
    assert_eq!(disabled, 0, "{body}");
}

/// The status names plugins and the restart stops the service: both the administrator's. A
/// capture token restarts only through its own route, and only with `capture:server_update`.
#[tokio::test]
async fn the_restart_is_refused_without_its_scope() {
    let directory = tempfile::tempdir().expect("tempdir");
    // With the login on: without it a caller on this machine holds every scope.
    let harness = common::harness(
        directory.path(),
        common::Options::default().parked().login(),
    )
    .await;
    let plans = installed_as(
        &harness,
        directory.path(),
        rd_update::InstallKind::Portable,
        by_hand(),
    );
    let (status, body) = get_with_bearer(&harness.router, STATUS, READ_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = post_with_bearer(&harness.router, STATUS, READ_BEARER, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    let (status, body) = post_with_bearer(
        &harness.router,
        "/api/v1/capture/server-update/restart",
        CAPTURE_BEARER,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(plans.lock().expect("plans").is_empty());
    assert!(!harness.state.restart.restarting());
    let (status, read) = get_with_bearer(
        &harness.router,
        "/api/v1/capture/server-update",
        CAPTURE_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["restart"]["pending"], false, "{read}");
    assert_eq!(read["may_install"], false, "{read}");
    let (status, body) = get_with_bearer(&harness.router, STATUS, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// systemd and a container restart the service on exit code 75; anything started by hand comes
/// back through the relauncher.
#[tokio::test]
async fn the_way_follows_the_install_kind_and_the_environment() {
    let cases = [
        (
            rd_update::InstallKind::Deb,
            systemd(),
            "supervisor",
            Some("systemd"),
        ),
        (
            rd_update::InstallKind::Docker,
            by_hand(),
            "supervisor",
            Some("container"),
        ),
        (rd_update::InstallKind::Portable, by_hand(), "self", None),
        (rd_update::InstallKind::Msi, by_hand(), "self", None),
        (
            rd_update::InstallKind::Deb,
            Environment {
                systemd: false,
                relaunchable: false,
            },
            "manual",
            None,
        ),
    ];
    for (kind, environment, how, supervisor) in cases {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = parked_harness(directory.path()).await;
        installed_as(&harness, directory.path(), kind, environment);
        let (_, body) = get_json(&harness.router, STATUS).await;
        assert_eq!(body["how"], how, "{kind:?}: {body}");
        assert_eq!(body["supervisor"].as_str(), supervisor, "{kind:?}: {body}");
    }
}

/// Under systemd the restart is the ordinary stop with exit code 75, recorded as a stop request
/// with `restart` set and announced; a second one is refused while it runs.
#[tokio::test]
async fn under_a_supervisor_the_restart_stops_the_service_with_exit_code_75() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let plans = installed_as(
        &harness,
        directory.path(),
        rd_update::InstallKind::Deb,
        systemd(),
    );
    let (status, body) = post_json(&harness.router, STATUS, json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["how"], "supervisor", "{body}");
    assert_eq!(body["supervisor"], "systemd", "{body}");
    assert!(
        harness.state.shutdown.is_cancelled(),
        "the graceful stop began"
    );
    assert_eq!(
        harness.state.restart.exit_code(),
        Some(rd_update::RESTART_EXIT_CODE)
    );
    assert_eq!(rd_update::RESTART_EXIT_CODE, 75);
    assert!(plans.lock().expect("plans").is_empty(), "no relauncher");

    let (_, status_body) = get_json(&harness.router, STATUS).await;
    assert_eq!(status_body["restarting"], true, "{status_body}");
    assert_eq!(status_body["can_restart"], false, "{status_body}");
    assert_eq!(
        status_body["blocked_reason"], "restart.already_restarting",
        "{status_body}"
    );
    let (status, again) = post_json(&harness.router, STATUS, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["code"], "restart.already_restarting", "{again}");

    let stops: Vec<_> = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .filter(|record| record.action == rd_core::AuditAction::ServiceStopRequested)
        .collect();
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(
        stops[0].details.get("restart").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        stops[0].details.get("how").map(String::as_str),
        Some("supervisor")
    );
}

/// Started by hand, the relauncher gets the plan: this version, this process, its data folder;
/// the service itself does not stop on its own. A relauncher that cannot start leaves it running
/// and a restart possible again.
#[tokio::test]
async fn by_hand_the_relauncher_gets_the_plan_and_a_failed_one_is_taken_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let failing: Relauncher = Arc::new(|_: &RestartPlan| -> anyhow::Result<()> {
        anyhow::bail!("the test refuses to start a relauncher")
    });
    harness.state.restart.use_environment(by_hand(), failing);
    harness.state.updates.use_installation(
        rd_update::InstallKind::Portable,
        directory.path().to_path_buf(),
        Arc::new(|_: &Journal| -> anyhow::Result<()> { Ok(()) }),
    );
    let (status, body) = post_json(&harness.router, STATUS, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "restart.relaunch_failed", "{body}");
    assert!(!harness.state.restart.restarting(), "taken back");
    assert!(!harness.state.shutdown.is_cancelled());

    let plans = installed_as(
        &harness,
        directory.path(),
        rd_update::InstallKind::Portable,
        by_hand(),
    );
    let (status, body) = post_json(&harness.router, STATUS, json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["how"], "self", "{body}");
    let plans = plans.lock().expect("plans");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].version, env!("CARGO_PKG_VERSION"));
    assert_eq!(plans[0].service_pid, std::process::id());
    assert_eq!(plans[0].data_dir, harness.state.updates.data_dir());
    assert_eq!(harness.state.restart.exit_code(), None);
    assert!(
        !harness.state.shutdown.is_cancelled(),
        "the relauncher stops it"
    );
}

/// Saves the settings document with `changes` over the current one.
async fn set(harness: &common::Harness, changes: serde_json::Value) {
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    settings["admin_login_disabled"] = json!(true);
    for (key, value) in changes.as_object().expect("object") {
        settings[key] = value.clone();
    }
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Off it never restarts; on, nothing pending restarts nothing; pending, it waits out the quiet
/// period, a download at work and the window, then restarts as the service's own decision.
#[tokio::test]
async fn the_automatic_restart_waits_for_quiet_and_the_window() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    installed_as(
        &harness,
        directory.path(),
        rd_update::InstallKind::Deb,
        systemd(),
    );
    let now = Utc::now();
    let mut restarter = AutoRestarter::default();
    assert_eq!(
        restarter.tick(&harness.state, now).await,
        Tick::Waiting(Wait::Off)
    );
    set(&harness, json!({ "restart_when_needed": true })).await;
    assert_eq!(
        restarter.tick(&harness.state, now).await,
        Tick::Waiting(Wait::NothingPending)
    );

    install_package(directory.path(), "1.0.0");
    switch_off(&harness).await;
    let download =
        crate::plugin_versions::pinned_download_of(&harness.database, PLUGIN, "1.0.0").await;
    harness
        .database
        .transition_download(download, rd_core::DownloadState::Resolving)
        .await
        .expect("working");
    let mut restarter = AutoRestarter::default();
    for minutes in [0, 10] {
        assert_eq!(
            restarter
                .tick(&harness.state, now + Duration::minutes(minutes))
                .await,
            Tick::Waiting(Wait::Busy)
        );
    }
    harness
        .database
        .transition_download(download, rd_core::DownloadState::Paused)
        .await
        .expect("paused");

    // Opens an hour from now, in UTC.
    let minute = u16::try_from(now.hour() * 60 + now.minute()).expect("minute");
    set(
        &harness,
        json!({
            "bandwidth_timezone": "UTC",
            "update_auto_install_window": {
                "start_minute": (minute + 60) % 1440,
                "end_minute": (minute + 120) % 1440
            }
        }),
    )
    .await;
    assert_eq!(
        restarter
            .tick(&harness.state, now + Duration::minutes(11))
            .await,
        Tick::Waiting(Wait::Settling)
    );
    assert_eq!(
        restarter
            .tick(&harness.state, now + Duration::minutes(17))
            .await,
        Tick::Waiting(Wait::OutsideWindow)
    );
    assert!(!harness.state.shutdown.is_cancelled());
    assert_eq!(
        restarter
            .tick(&harness.state, now + Duration::minutes(61))
            .await,
        Tick::Started("supervisor".to_owned())
    );
    assert!(harness.state.shutdown.is_cancelled());
    let stops: Vec<_> = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .filter(|record| record.action == rd_core::AuditAction::ServiceStopRequested)
        .collect();
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(stops[0].actor_kind, rd_core::AuditActorKind::System);
    assert_eq!(
        stops[0].details.get("automatic").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        restarter
            .tick(&harness.state, now + Duration::minutes(62))
            .await,
        Tick::Waiting(Wait::Installing),
        "one restart at a time"
    );
}
