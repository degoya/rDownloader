//! Crash and restart of the plugin transfer runner (RD-180-12, recovery matrix).
//!
//! The runner is driven twice over the same database and folder, the second time with fresh
//! backends: nothing survives from the first attempt but the part file and the rows, which is
//! what survives a killed process. Named `_contract` because it drives the reference backend
//! component, which the `no-components` profile leaves out by that suffix.

#![cfg(feature = "failpoints")]

mod support;

use std::{path::PathBuf, sync::Arc, time::Duration};

use rd_core::failpoint::FailpointGuard;
use rd_plugin_host::{PluginManifest, TransferBackend};
use tokio_util::sync::CancellationToken;

/// Every byte says where it belongs, so a resume at the wrong offset cannot look right.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
        address_policy: None,
    }
}

fn runner(
    manifest_toml: &str,
    component: &[u8],
    database: &rd_db::Database,
) -> Arc<dyn rd_scheduler::ExternalRunner> {
    let manifest: PluginManifest = toml::from_str(manifest_toml).expect("manifest");
    let backends = rd_plugin_transfer::TransferBackends::for_test(
        vec![Arc::new(
            TransferBackend::new(manifest, component).expect("backend"),
        )],
        true,
    );
    rd_plugin_transfer::build(backends, database.clone(), Vec::new())
}

/// One plugin transfer row over a served payload, and everything a case looks at afterwards.
struct Case {
    component: Vec<u8>,
    expected: Vec<u8>,
    _server: support::Server,
    manifest: String,
    _directory: tempfile::TempDir,
    database: rd_db::Database,
    destination: PathBuf,
    package: rd_core::DownloadPackage,
    file: rd_core::DownloadFile,
    part: PathBuf,
}

impl Case {
    /// ~120 chunks at 5 ms each, so a stop lands well inside the transfer.
    async fn new() -> Self {
        let component = support::component();
        let expected = payload(1_000_000);
        let server = support::serve(expected.clone(), 8 * 1024, Duration::from_millis(5)).await;
        let manifest = support::manifest(server.address.port());
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let destination = directory.path().join("package");
        let package = database
            .create_package(rd_db::NewPackage {
                id: rd_core::PackageId::new(),
                name: "plugin transfer".to_owned(),
                destination: destination.display().to_string(),
                category_id: None,
                priority: rd_core::DownloadPriority::default(),
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let url = format!("example+tcp://127.0.0.1:{}/file.bin", server.address.port());
        let file = database
            .create_download(rd_db::NewDownload {
                id: rd_core::DownloadId::new(),
                package_id: package.id,
                source: url.parse().expect("url"),
                file_name: "file.bin".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::default(),
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Plugin,
                media: None,
                remote_credential_id: None,
                mirror_group: None,
                enrichment: Vec::new(),
                replay: None,
                secret_fragment: None,
            })
            .await
            .expect("download");
        let part = destination
            .join(".rdownloader")
            .join(format!("{}.part", file.id));
        Self {
            component,
            expected,
            _server: server,
            manifest,
            _directory: directory,
            database,
            destination,
            package,
            file,
            part,
        }
    }

    /// The first attempt, stopped at `point`; `stop` cancels it once two chunks are on disk.
    async fn crash_at(&self, point: &str, stop: bool) {
        let cancellation = CancellationToken::new();
        if stop {
            // Stop once two chunks are on disk, not after a fixed time (see the contract test).
            let stop = cancellation.clone();
            let watched = self.part.clone();
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                while tokio::time::Instant::now() < deadline {
                    let written = tokio::fs::metadata(&watched)
                        .await
                        .map_or(0, |meta| meta.len());
                    if written >= 2 * 8 * 1024 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                stop.cancel();
            });
        }
        let guard = FailpointGuard::once(point);
        let first = runner(&self.manifest, &self.component, &self.database);
        let outcome = rd_scheduler::ExternalRunner::run(
            first.as_ref(),
            &self.file,
            &self.package,
            cancellation,
            limits(),
        )
        .await;
        assert!(outcome.is_err(), "the attempt ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }

    /// The restart on `manifest`: fresh backends, the same rows and folder. It has to finish
    /// with the source's bytes and leave no pin behind.
    async fn restart_finishes(&self, manifest: &str) {
        let second = runner(manifest, &self.component, &self.database);
        let outcome = rd_scheduler::ExternalRunner::run(
            second.as_ref(),
            &self.file,
            &self.package,
            CancellationToken::new(),
            limits(),
        )
        .await
        .expect("second attempt");
        match outcome {
            rd_scheduler::RunOutcome::Completed { final_name } => {
                assert_eq!(final_name, "file.bin");
            }
            other => panic!("expected the restarted transfer to complete, got {other:?}"),
        }
        let promoted = tokio::fs::read(self.destination.join("file.bin"))
            .await
            .expect("the promoted file");
        assert_eq!(promoted.len(), self.expected.len());
        assert!(
            promoted == self.expected,
            "the finished file differs from the source"
        );
        assert!(!self.part.exists(), "the part file survived the promotion");
        assert!(
            self.database
                .plugin_transfer(self.file.id)
                .await
                .expect("read checkpoint")
                .is_none()
        );
    }

    /// The pin the first attempt left: this manifest's version, and no checkpoint.
    async fn assert_pinned_without_checkpoint(&self) {
        let pin = self
            .database
            .plugin_transfer(self.file.id)
            .await
            .expect("read checkpoint")
            .expect("the version is pinned before the first byte");
        assert_eq!(pin.plugin_version, "0.7.0");
        assert!(pin.checkpoint.is_none(), "no checkpoint was ever saved");
    }
}

/// `plugin_transfer.before_checkpoint_saved`: the backend stopped with bytes on disk, and the
/// runner never recorded the checkpoint. The pin saved before the first byte binds the bytes to
/// the version that wrote them (RD-1120-18).
#[tokio::test]
async fn a_transfer_stopped_before_its_checkpoint_was_saved_continues_from_the_part_file() {
    let case = Case::new().await;
    case.crash_at("plugin_transfer.before_checkpoint_saved", true)
        .await;

    // What the stop left: bytes that are exactly the source's first ones, and the pin.
    let partial = tokio::fs::read(&case.part)
        .await
        .expect("the part file stays");
    assert!(!partial.is_empty(), "the stop kept nothing");
    assert!(
        partial.len() < case.expected.len(),
        "it stopped too late to prove anything"
    );
    assert_eq!(
        partial,
        case.expected[..partial.len()],
        "the part file holds invented bytes"
    );
    case.assert_pinned_without_checkpoint().await;

    case.restart_finishes(&case.manifest).await;
}

/// `plugin_transfer.after_pin_saved`: the pin is written, no byte is. The restart runs on the
/// pinned version.
#[tokio::test]
async fn a_transfer_stopped_after_its_pin_finishes_on_the_pinned_version() {
    let case = Case::new().await;
    case.crash_at("plugin_transfer.after_pin_saved", false)
        .await;

    assert_eq!(
        rd_files::existing_bytes(&case.part).await,
        0,
        "a byte arrived before the pin"
    );
    case.assert_pinned_without_checkpoint().await;

    case.restart_finishes(&case.manifest).await;
}

/// `plugin_transfer.after_pin_saved`, then an upgrade took the pinned version away. A pin with
/// no checkpoint and no bytes has nothing only that build could continue, so the transfer
/// begins anew on the newest backend instead of failing with `plugin.pinned_version_missing`.
#[tokio::test]
async fn a_transfer_stopped_after_its_pin_begins_anew_when_its_version_is_gone() {
    let case = Case::new().await;
    case.crash_at("plugin_transfer.after_pin_saved", false)
        .await;
    case.assert_pinned_without_checkpoint().await;

    let upgraded = case
        .manifest
        .replace(r#"version = "0.7.0""#, r#"version = "0.8.0""#);
    assert_ne!(upgraded, case.manifest, "the upgrade changed nothing");
    case.restart_finishes(&upgraded).await;
}
