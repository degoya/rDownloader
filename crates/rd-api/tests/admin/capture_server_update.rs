//! The service's update in the capture agent's tray (RD-1240-25): every agent reads whether there
//! is one and how it is installed; only an agent paired with `capture:server_update` installs it,
//! through the install the web interface starts, and an installation that does not install
//! itself is refused with its code there too.

use crate::common;
use crate::updates::{ARTIFACT_BYTES, installable};

use axum::http::StatusCode;
use common::{CAPTURE_BEARER, get_json, get_with_bearer, post_with_bearer};
use serde_json::json;
use sha2::{Digest, Sha256};

const READ: &str = "/api/v1/capture/server-update";
const INSTALL: &str = "/api/v1/capture/server-update/install";

/// A bearer for an agent paired with `scopes` on top of `capture:*`.
async fn agent_with(database: &rd_db::Database, bearer: &str, scopes: &[&str]) -> String {
    let mut held = vec![rd_core::CAPTURE_SCOPE.to_owned()];
    held.extend(scopes.iter().map(|scope| (*scope).to_owned()));
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Tray".to_owned(),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            held,
        )
        .await
        .expect("token");
    bearer.to_owned()
}

#[tokio::test]
async fn every_agent_reads_the_offer_and_whether_it_may_install_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, _) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let (status, plain) = get_with_bearer(&harness.router, READ, CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    assert_eq!(plain["available"]["version"], "99.0.0", "{plain}");
    assert_eq!(plain["available"]["action"], "install", "{plain}");
    assert_eq!(plain["may_install"], false, "{plain}");
    assert!(plain["install"].is_null(), "{plain}");
    // Nothing of the administration around it: no channel, no address, no schedule.
    for absent in ["channel", "download_url", "next_check_at", "install_kind"] {
        assert!(!plain.to_string().contains(absent), "{absent}: {plain}");
    }

    let bearer = agent_with(
        &harness.database,
        "test-server-update-bearer",
        &[rd_core::CAPTURE_SERVER_UPDATE_SCOPE],
    )
    .await;
    let (status, allowed) = get_with_bearer(&harness.router, READ, &bearer).await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    assert_eq!(allowed["may_install"], true, "{allowed}");
}

/// Neither a plain capture token nor one with queue control installs the update: the refusal
/// names the scope it lacks, and nothing reaches the updater.
#[tokio::test]
async fn without_the_server_update_right_the_install_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let controlling = agent_with(
        &harness.database,
        "test-queue-control-bearer",
        &[rd_core::CAPTURE_QUEUE_SCOPE],
    )
    .await;
    for bearer in [CAPTURE_BEARER, controlling.as_str()] {
        let (status, refused) = post_with_bearer(&harness.router, INSTALL, bearer, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
        assert_eq!(refused["code"], "auth.scope_insufficient", "{refused}");
        assert_eq!(
            refused["params"]["scope"], "capture:server_update",
            "{refused}"
        );
    }
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert!(body["install"].is_null(), "{body}");
    assert!(launched.lock().expect("launched").is_empty());
}

/// With the right the tray's click is the web interface's "Install and restart": the same steps
/// up to the hand-over to the updater, followed through the agent's own route.
#[tokio::test]
async fn with_the_right_the_agent_installs_through_the_same_install() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let bearer = agent_with(
        &harness.database,
        "test-server-update-bearer",
        &[rd_core::CAPTURE_SERVER_UPDATE_SCOPE],
    )
    .await;
    let (status, started) = post_with_bearer(&harness.router, INSTALL, &bearer, json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["target_version"], "99.0.0", "{started}");

    let mut reached = serde_json::Value::Null;
    for _ in 0..200 {
        let (_, body) = get_with_bearer(&harness.router, READ, &bearer).await;
        if body["install"]["state"] == "restarting" {
            reached = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(reached["install"]["target_version"], "99.0.0", "{reached}");
    let journal = launched.lock().expect("launched")[0].clone();
    assert_eq!(journal.plan.target_version, "99.0.0");

    // A second click while it runs is the service's own refusal, with its code.
    let (status, again) = post_with_bearer(&harness.router, INSTALL, &bearer, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["code"], "update.install_running", "{again}");
}

/// A package manager's installation offers its command, and the install is refused there as it
/// is from the web interface.
#[tokio::test]
async fn a_package_manager_installation_offers_its_command_and_no_install() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Deb,
        ARTIFACT_BYTES,
    )
    .await;
    let bearer = agent_with(
        &harness.database,
        "test-server-update-bearer",
        &[rd_core::CAPTURE_SERVER_UPDATE_SCOPE],
    )
    .await;
    let (_, offer) = get_with_bearer(&harness.router, READ, &bearer).await;
    assert_eq!(offer["available"]["action"], "command", "{offer}");
    assert!(offer["available"]["command"].is_string(), "{offer}");

    let (status, refused) = post_with_bearer(&harness.router, INSTALL, &bearer, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "update.install_unsupported", "{refused}");
    assert!(launched.lock().expect("launched").is_empty());
}

/// The right is chosen at pairing, off unless asked for, and independent of queue control.
#[tokio::test]
async fn pairing_grants_the_server_update_right_only_when_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;

    let (status, plain) =
        common::post_json(router, "/api/v1/capture/pair", json!({ "label": "Laptop" })).await;
    assert_eq!(status, StatusCode::CREATED, "{plain}");
    assert_eq!(plain["token"]["scopes"], json!(["capture:*"]));

    let (status, updating) = common::post_json(
        router,
        "/api/v1/capture/pair",
        json!({ "label": "Desktop", "server_update": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{updating}");
    assert_eq!(
        updating["token"]["scopes"],
        json!(["capture:*", "capture:server_update"])
    );
    let bearer = updating["bearer"].as_str().expect("bearer");
    let (status, read) = get_with_bearer(router, READ, bearer).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["may_install"], true, "{read}");
    // The right is the install alone: queue control stays where it was chosen.
    let (status, _) =
        post_with_bearer(router, "/api/v1/capture/queue/resume", bearer, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
