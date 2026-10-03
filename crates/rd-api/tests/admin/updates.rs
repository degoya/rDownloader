//! The application update check over REST (RD-180-01).
//!
//! The manifest rules themselves are `crates/rd-update`'s tests. What is checked here is the
//! contract around them: the status document, what a check finds when the manifest is good,
//! replayed or forged, and the settings that steer it. Manifests are served from memory and
//! signed with a test key, so nothing reaches GitHub.
//!
//! The self-update (RD-180-02) is checked up to the hand-over: the download held to the signed
//! digest, the backup before it, the journal the updater reads, and the status it then shows;
//! and the download ahead of the install, whose verified file the install then takes.
//! The updater itself is `rd_update::install`'s tests and `.github/workflows/self-update.yml`.

use crate::common;

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{get_json, post_json, put_json, test_harness};
use rd_update::{
    Artifact, Channel, MemoryFetcher, SigningKey, Sources, Target, TrustStore, UpdateManifest,
    install::Journal,
};
use serde_json::json;
use sha2::{Digest, Sha256};

const KEY_ID: &str = "rdownloader-update-v1";
const STABLE: &str = "https://updates.example.test/stable.json";

fn key() -> SigningKey {
    SigningKey::from_bytes(&[42; 32])
}

fn manifest(version: &str, sequence: u64) -> UpdateManifest {
    let now = Utc::now();
    UpdateManifest {
        schema_version: 1,
        sequence,
        issued_at: now - Duration::minutes(5),
        not_after: now + Duration::days(30),
        channel: Channel::Stable,
        version: version.to_owned(),
        released_at: now - Duration::minutes(5),
        notes: "Added\n- A thing".to_owned(),
        artifacts: vec![Artifact {
            platform: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            kind: "archive".to_owned(),
            url: "https://updates.example.test/rdownloader-linux-x86_64.tar.gz".to_owned(),
            sha256: "cd".repeat(32),
            size: 4096,
        }],
        schema_change: None,
    }
}

fn signed(manifest: &UpdateManifest, key: &SigningKey) -> Vec<u8> {
    rd_update::manifest::sign(KEY_ID, key, manifest).expect("sign")
}

/// A harness whose update check reads `fetcher` under the test key.
async fn served(directory: &std::path::Path) -> (common::Harness, MemoryFetcher) {
    let harness = test_harness(directory).await;
    let fetcher = MemoryFetcher::new();
    let trust = TrustStore::new();
    trust
        .trust(KEY_ID.to_owned(), key().verifying_key())
        .expect("trust");
    harness.state.updates.use_source(
        Arc::new(fetcher.clone()),
        Sources {
            stable: url::Url::parse(STABLE).expect("url"),
            releases: url::Url::parse("https://updates.example.test/releases").expect("url"),
            download_prefix: "https://updates.example.test/".to_owned(),
        },
        trust,
    );
    (harness, fetcher)
}

/// A fresh installation has checked nothing and offers nothing, and says how it was installed.
#[tokio::test]
async fn a_fresh_installation_reports_its_version_and_nothing_offered() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["current_version"], env!("CARGO_PKG_VERSION"), "{body}");
    assert_eq!(body["configured"], true, "{body}");
    assert_eq!(body["check_enabled"], true, "{body}");
    // `stable`, or `beta` when this build is a pre-release (rd_update::settings::default_channel).
    let channel = rd_update::UpdateSettings::default().update_channel;
    assert_eq!(body["channel"], channel.as_str(), "{body}");
    assert_eq!(body["interval_hours"], 24, "{body}");
    assert!(body["install_kind"].is_string(), "{body}");
    assert!(body["last_checked_at"].is_null(), "{body}");
    assert!(body["available"].is_null(), "{body}");
    assert_eq!(body["checking"], false, "{body}");
}

#[tokio::test]
async fn a_check_finds_a_newer_signed_release() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(STABLE, signed(&manifest("99.0.0", 10), &key()));
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["error_code"].is_null(), "{body}");
    assert!(body["last_checked_at"].is_string(), "{body}");
    let offer = &body["available"];
    assert_eq!(offer["version"], "99.0.0", "{body}");
    assert_eq!(offer["channel"], "stable", "{body}");
    assert_eq!(offer["notes"], "Added\n- A thing", "{body}");
    assert_eq!(
        offer["release_url"], "https://github.com/degoya/rDownloader/releases/tag/v99.0.0",
        "{body}"
    );
    assert!(
        matches!(offer["action"].as_str(), Some("download" | "command")),
        "{body}"
    );
    // The result survives into the next read, which fetches nothing.
    let requests = fetcher.requests().len();
    let (_, again) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(again["available"]["version"], "99.0.0", "{again}");
    assert_eq!(fetcher.requests().len(), requests);
}

/// Not the version running, not an older one.
#[tokio::test]
async fn an_older_release_is_not_offered() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(STABLE, signed(&manifest("0.1.0", 10), &key()));
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["available"].is_null(), "{body}");
    assert!(body["error_code"].is_null(), "{body}");
}

/// A newer version reaches a rule that asks for `update_available`, once however often the
/// check finds it again (RD-190-19).
#[tokio::test]
async fn an_available_version_is_announced_once_however_often_it_is_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(STABLE, signed(&manifest("99.0.0", 10), &key()));
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
        json!({ "name": "updates", "target_id": target["id"], "events": ["update_available"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    for _ in 0..3 {
        let (status, body) =
            post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["available"]["version"], "99.0.0", "{body}");
    }
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
    assert_eq!(
        deliveries[0].event,
        rd_notify::NotificationEvent::UpdateAvailable
    );
    assert!(deliveries[0].title.contains("99.0.0"), "{deliveries:?}");
}

/// A forged manifest is reported and offers nothing.
#[tokio::test]
async fn a_forged_manifest_is_reported_and_offers_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(
        STABLE,
        signed(&manifest("99.0.0", 10), &SigningKey::from_bytes(&[1; 32])),
    );
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["error_code"], "update.bad_signature", "{body}");
    assert!(body["available"].is_null(), "{body}");
}

/// Once a manifest was accepted, an older one served in its place is a replay.
#[tokio::test]
async fn a_replayed_manifest_is_refused_after_a_newer_one_was_accepted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(STABLE, signed(&manifest("99.0.0", 10), &key()));
    post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    fetcher.serve(STABLE, signed(&manifest("98.0.0", 9), &key()));
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["error_code"], "update.stale", "{body}");
    // What was verified before stays offered.
    assert_eq!(body["available"]["version"], "99.0.0", "{body}");
}

#[tokio::test]
async fn nothing_published_yet_is_its_own_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, _fetcher) = served(directory.path()).await;
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["error_code"], "update.not_published", "{body}");
}

/// The channel and the interval are settings; a value the check cannot use is refused.
#[tokio::test]
async fn the_update_settings_are_validated_and_shown() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    // As in `managed_tools`: the harness runs with the login off, and a saved document that
    // says otherwise would switch it on under the following requests.
    settings["admin_login_disabled"] = json!(true);
    assert_eq!(
        settings["update_channel"],
        rd_update::UpdateSettings::default().update_channel.as_str(),
        "{settings}"
    );
    assert_eq!(settings["update_check_enabled"], true, "{settings}");
    assert_eq!(settings["update_check_interval_hours"], 24, "{settings}");

    let mut nightly = settings.clone();
    nightly["update_channel"] = json!("nightly");
    let (status, body) = put_json(&harness.router, "/api/v1/settings", nightly).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.update_channel_invalid", "{body}");

    let mut never = settings.clone();
    never["update_check_interval_hours"] = json!(0);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", never).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.update_interval_invalid", "{body}");

    let mut beta = settings.clone();
    beta["update_channel"] = json!("beta");
    beta["update_check_enabled"] = json!(false);
    beta["update_check_interval_hours"] = json!(72);
    let (status, body) = put_json(&harness.router, "/api/v1/settings", beta).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, status_body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(status_body["channel"], "beta", "{status_body}");
    assert_eq!(status_body["check_enabled"], false, "{status_body}");
    assert_eq!(status_body["interval_hours"], 72, "{status_body}");
    assert!(status_body["next_check_at"].is_null(), "{status_body}");
}

// ---- RD-180-02: installing the offered update ----

const ARTIFACT_BYTES: &[u8] = b"the next version's archive";

/// A harness whose update check offers `99.0.0` with an artifact for this platform, installed as
/// `kind` from a folder of its own, with an updater that records the journal instead of running.
async fn installable(
    directory: &std::path::Path,
    kind: rd_update::InstallKind,
    artifact_body: &[u8],
) -> (common::Harness, Arc<std::sync::Mutex<Vec<Journal>>>) {
    let (harness, launched, _) = installable_from(directory, kind, artifact_body).await;
    (harness, launched)
}

/// [`installable`], with the fetcher that serves the artifact, which counts its requests.
async fn installable_from(
    directory: &std::path::Path,
    kind: rd_update::InstallKind,
    artifact_body: &[u8],
) -> (
    common::Harness,
    Arc<std::sync::Mutex<Vec<Journal>>>,
    MemoryFetcher,
) {
    let (harness, fetcher) = served(directory).await;
    let target = Target::current(kind);
    let url = format!(
        "https://updates.example.test/rdownloader-{}-{}.{}",
        target.platform,
        target.arch,
        if cfg!(windows) { "zip" } else { "tar.gz" }
    );
    let mut release = manifest("99.0.0", 10);
    release.artifacts = vec![Artifact {
        platform: target.platform.to_owned(),
        arch: target.arch.to_owned(),
        kind: target.kind.to_owned(),
        url: url.clone(),
        sha256: hex::encode(Sha256::digest(ARTIFACT_BYTES)),
        size: ARTIFACT_BYTES.len() as u64,
    }];
    fetcher.serve(STABLE, signed(&release, &key()));
    fetcher.serve(&url, artifact_body.to_vec());
    let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&launched);
    let program = directory.join("program");
    std::fs::create_dir_all(&program).expect("program folder");
    harness.state.updates.use_installation(
        kind,
        program,
        Arc::new(move |journal: &Journal| -> anyhow::Result<()> {
            recorded.lock().expect("launched").push(journal.clone());
            Ok(())
        }),
    );
    let (status, body) = post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (harness, launched, fetcher)
}

/// Reads the status until the install reaches `state`, or fails the test.
async fn install_reaches(harness: &common::Harness, state: &str) -> serde_json::Value {
    for _ in 0..200 {
        let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
        if body["install"]["state"] == state {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    panic!("the install never reached {state}: {body}");
}

#[tokio::test]
async fn a_portable_installation_downloads_backs_up_and_hands_over_to_the_updater() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(body["available"]["action"], "install", "{body}");
    assert_eq!(body["available"]["rollback_available"], true, "{body}");
    assert!(body["install"].is_null(), "{body}");

    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["target_version"], "99.0.0", "{body}");
    let body = install_reaches(&harness, "restarting").await;
    assert_eq!(
        body["install"]["from_version"],
        env!("CARGO_PKG_VERSION"),
        "{body}"
    );

    let journal = launched.lock().expect("launched")[0].clone();
    let plan = &journal.plan;
    assert_eq!(journal.phase, rd_update::install::Phase::Handed);
    assert_eq!(plan.kind, rd_update::InstallKind::Portable);
    assert_eq!(plan.target_version, "99.0.0");
    assert_eq!(
        std::fs::read(&plan.artifact).expect("artifact"),
        ARTIFACT_BYTES
    );
    assert_eq!(plan.sha256, hex::encode(Sha256::digest(ARTIFACT_BYTES)));
    assert!(plan.install_dir.ends_with("program"), "{plan:?}");
    assert!(plan.install_dir.is_absolute() && plan.data_dir.is_absolute());
    // The checked copy of the backup before the update is what a rollback puts back.
    let copy = plan.database_copy.as_ref().expect("copy");
    assert!(copy.is_file(), "{copy:?}");
    assert!(
        copy.starts_with(plan.data_dir.join("pre-update")),
        "{copy:?}"
    );
    assert_eq!(plan.service_pid, std::process::id());
    // The journal the updater reads is on disk, not only handed to it.
    let stored = Journal::read(&plan.data_dir)
        .expect("read")
        .expect("written");
    assert_eq!(stored.plan, *plan);

    let started = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .filter(|record| record.action == rd_core::AuditAction::UpdateInstallStarted)
        .count();
    assert_eq!(started, 1);

    // A second install while this one runs is refused.
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.install_running", "{body}");

    // What the updater writes is what the status shows: here, a rollback and its reason.
    let mut ended = stored;
    ended
        .end(
            rd_update::install::Phase::RolledBack,
            "update.health_timeout",
            "the test says so",
        )
        .expect("end");
    let body = install_reaches(&harness, "rolled_back").await;
    assert_eq!(body["install"]["reason"], "update.health_timeout", "{body}");
}

#[tokio::test]
async fn a_download_that_is_not_the_signed_one_is_never_handed_over() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        b"something else entirely!!!",
    )
    .await;
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let body = install_reaches(&harness, "failed").await;
    assert_eq!(
        body["install"]["reason"], "update.digest_mismatch",
        "{body}"
    );
    assert!(launched.lock().expect("launched").is_empty());
}

#[tokio::test]
async fn a_package_manager_installation_does_not_install_itself() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Deb,
        ARTIFACT_BYTES,
    )
    .await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(body["available"]["action"], "command", "{body}");
    assert!(body["available"]["rollback_available"].is_null(), "{body}");
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.install_unsupported", "{body}");
    assert_eq!(body["params"]["kind"], "deb", "{body}");
    assert!(launched.lock().expect("launched").is_empty());
}

#[tokio::test]
async fn nothing_offered_is_nothing_to_install() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, fetcher) = served(directory.path()).await;
    fetcher.serve(STABLE, signed(&manifest("0.1.0", 10), &key()));
    harness.state.updates.use_installation(
        rd_update::InstallKind::Portable,
        directory.path().to_path_buf(),
        Arc::new(|_: &Journal| -> anyhow::Result<()> { Ok(()) }),
    );
    post_json(&harness.router, "/api/v1/system/update/check", json!({})).await;
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.none_available", "{body}");
}

// ---- RD-180-02, owner 2026-10-01: the download ahead of the install ----

/// Reads the status until the background download reaches `state`, or fails the test.
async fn download_reaches(harness: &common::Harness, state: &str) -> serde_json::Value {
    for _ in 0..200 {
        let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
        if body["download"]["state"] == state {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    panic!("the download never reached {state}: {body}");
}

/// How often the artifact itself was fetched, not the manifests.
fn artifact_fetches(fetcher: &MemoryFetcher) -> usize {
    fetcher
        .requests()
        .iter()
        .filter(|url| url.contains("/rdownloader-"))
        .count()
}

#[tokio::test]
async fn the_update_downloads_in_the_background_and_the_install_takes_that_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched, fetcher) = installable_from(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert!(body["download"].is_null(), "{body}");

    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/download", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["version"], "99.0.0", "{body}");
    assert_eq!(body["total_bytes"], ARTIFACT_BYTES.len(), "{body}");
    let body = download_reaches(&harness, "ready").await;
    assert_eq!(
        body["download"]["received_bytes"],
        ARTIFACT_BYTES.len(),
        "{body}"
    );
    // Downloading is not installing: nothing was handed over, nothing is in progress.
    assert!(body["install"].is_null(), "{body}");
    assert!(launched.lock().expect("launched").is_empty());
    assert_eq!(artifact_fetches(&fetcher), 1);

    // A second click finds the verified file instead of fetching it again.
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/download", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    download_reaches(&harness, "ready").await;
    assert_eq!(artifact_fetches(&fetcher), 1);

    // And the install installs that file.
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    install_reaches(&harness, "restarting").await;
    assert_eq!(artifact_fetches(&fetcher), 1);
    let journal = launched.lock().expect("launched")[0].clone();
    assert_eq!(
        std::fs::read(&journal.plan.artifact).expect("artifact"),
        ARTIFACT_BYTES
    );
}

#[tokio::test]
async fn a_kept_download_that_changed_on_disk_is_fetched_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched, fetcher) = installable_from(
        directory.path(),
        rd_update::InstallKind::Portable,
        ARTIFACT_BYTES,
    )
    .await;
    post_json(&harness.router, "/api/v1/system/update/download", json!({})).await;
    download_reaches(&harness, "ready").await;
    let kept = rd_update::install::update_dir(&harness.state.updates.data_dir())
        .join(rd_update::install::DOWNLOAD_DIR);
    for file in std::fs::read_dir(&kept).expect("kept") {
        std::fs::write(file.expect("entry").path(), b"tampered with, same length").expect("write");
    }

    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/install", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    install_reaches(&harness, "restarting").await;
    assert_eq!(artifact_fetches(&fetcher), 2);
    let journal = launched.lock().expect("launched")[0].clone();
    assert_eq!(
        std::fs::read(&journal.plan.artifact).expect("artifact"),
        ARTIFACT_BYTES
    );
}

#[tokio::test]
async fn a_download_that_is_not_the_signed_one_fails_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, launched) = installable(
        directory.path(),
        rd_update::InstallKind::Portable,
        b"something else entirely!!!",
    )
    .await;
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/download", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let body = download_reaches(&harness, "failed").await;
    assert_eq!(
        body["download"]["reason"], "update.digest_mismatch",
        "{body}"
    );
    assert!(body["install"].is_null(), "{body}");
    assert!(launched.lock().expect("launched").is_empty());
}

#[tokio::test]
async fn a_package_manager_installation_does_not_download_in_the_background() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (harness, _launched, fetcher) = installable_from(
        directory.path(),
        rd_update::InstallKind::Deb,
        ARTIFACT_BYTES,
    )
    .await;
    let (status, body) =
        post_json(&harness.router, "/api/v1/system/update/download", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "update.install_unsupported", "{body}");
    assert_eq!(artifact_fetches(&fetcher), 0);
}

/// Opens the capture event stream as an agent that names itself with `user_agent`, or with
/// nothing at all, like an agent from before 1.9. The response is returned unread: the stream
/// stays open for as long as it is held.
async fn capture_stream(
    router: &axum::Router,
    user_agent: Option<&str>,
) -> axum::response::Response {
    use tower::ServiceExt;

    let mut request = axum::http::Request::builder()
        .uri("/api/v1/capture/events")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", common::CAPTURE_BEARER),
        );
    if let Some(user_agent) = user_agent {
        request = request.header(axum::http::header::USER_AGENT, user_agent);
    }
    let response = router
        .clone()
        .oneshot(request.body(axum::body::Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    response
}

/// The update view shows the version of each running capture agent beside the service's, and
/// says when the agent is older (RD-190-07). Without a running agent there is nothing to say.
#[tokio::test]
async fn the_status_names_the_running_capture_agents_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(body["capture_agents"], json!([]), "{body}");

    let current = capture_stream(
        &harness.router,
        Some(&format!(
            "rdownloader-capture/{}",
            env!("CARGO_PKG_VERSION")
        )),
    )
    .await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(
        body["capture_agents"],
        json!([{ "version": env!("CARGO_PKG_VERSION"), "outdated": false }]),
        "{body}"
    );
    drop(current);
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(
        body["capture_agents"],
        json!([]),
        "a closed stream is an agent that no longer runs: {body}"
    );

    let older = capture_stream(&harness.router, Some("rdownloader-capture/0.1.0")).await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(
        body["capture_agents"],
        json!([{ "version": "0.1.0", "outdated": true }]),
        "{body}"
    );
    drop(older);

    // An agent from before 1.9 names no version and is older by that alone.
    let _silent = capture_stream(&harness.router, None).await;
    let (_, body) = get_json(&harness.router, "/api/v1/system/update").await;
    assert_eq!(
        body["capture_agents"],
        json!([{ "version": null, "outdated": true }]),
        "{body}"
    );
}
