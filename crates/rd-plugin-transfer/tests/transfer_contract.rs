//! The transfer contract, exercised end to end against the reference backend.
//!
//! Every test here is about a promise the host makes to the user rather than to the plugin:
//! that a paused download continues where it stopped, that bytes already on disk are never
//! lost or duplicated, that a backend reaches only what its manifest names, and that the
//! speed limit applies to a protocol the core knows nothing about.

mod support;

use std::{sync::Arc, time::Duration};

use rd_plugin_host::{
    PluginManifest, TransferBackend, TransferJob, TransferOutcome, TransferState, TransferTarget,
};
use tokio_util::sync::CancellationToken;

/// Deterministic payload: every byte says where it belongs, so a resume that splices at the
/// wrong offset produces a mismatch instead of a plausible-looking file.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn backend(manifest_toml: &str, component: &[u8]) -> TransferBackend {
    let manifest: PluginManifest = toml::from_str(manifest_toml).expect("manifest");
    TransferBackend::new(manifest, component).expect("compile backend")
}

struct Attempt {
    part: std::path::PathBuf,
    cancellation: CancellationToken,
    _directory: tempfile::TempDir,
}

impl Attempt {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        Self {
            part: directory.path().join("download.part"),
            cancellation: CancellationToken::new(),
            _directory: directory,
        }
    }

    async fn state(
        &self,
        backend: &TransferBackend,
        committed: u64,
        total: Option<u64>,
    ) -> TransferState {
        let part = rd_files::PartFile::open(self.part.clone(), None)
            .await
            .expect("part file");
        TransferState::new(
            TransferTarget {
                part,
                committed,
                total,
            },
            self.cancellation.clone(),
            rd_limits::ScopedLimiter::unlimited(),
            backend.manifest().capabilities.net_stream.clone(),
            Arc::new(rd_http::tls_client_config(&[]).expect("tls")),
            Arc::new(|_, _| {}),
        )
        .allowing_local_targets()
    }

    async fn written(&self) -> Vec<u8> {
        tokio::fs::read(&self.part).await.unwrap_or_default()
    }
}

fn job(url: String, checkpoint: Option<Vec<u8>>) -> TransferJob {
    TransferJob {
        url,
        credential_ref: None,
        checkpoint,
    }
}

/// A download that is stopped part way through resumes from what is on disk, and the file it
/// ends up with is byte-for-byte the one the server served.
///
/// The new backend instance is the restart: nothing survives from the first attempt except
/// the part file and the checkpoint, which is exactly what survives a process exit.
#[tokio::test]
async fn a_stopped_transfer_resumes_from_disk_after_a_restart() {
    let component = support::component();
    // ~120 chunks at 5 ms each: the transfer runs for well over half a second once it starts.
    let expected = payload(1_000_000);
    let server = support::serve(expected.clone(), 8 * 1024, Duration::from_millis(5)).await;
    let manifest = support::manifest(server.address.port());
    let url = format!("example+tcp://127.0.0.1:{}/file.bin", server.address.port());

    let first = backend(&manifest, &component);
    let attempt = Attempt::new();
    let cancellation = attempt.cancellation.clone();
    // Stop once two chunks are on disk, not after a fixed time: on a GitHub runner the
    // component had not written its first byte 60 ms in, and the stop kept nothing (CI,
    // 2026-09-25). The deadline only ends a transfer that never starts.
    let part = attempt.part.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while tokio::time::Instant::now() < deadline {
            let written = tokio::fs::metadata(&part)
                .await
                .map_or(0, |meta| meta.len());
            if written >= 2 * 8 * 1024 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        cancellation.cancel();
    });
    let state = attempt.state(&first, 0, None).await;
    let stopped = first
        .run(state, job(url.clone(), None))
        .await
        .expect("first attempt");
    let TransferOutcome::Stopped {
        committed,
        checkpoint,
    } = stopped
    else {
        panic!("the transfer should have been stopped");
    };
    assert!(committed > 0, "the stop must keep what already arrived");
    assert!(
        committed < expected.len() as u64,
        "it stopped too late to prove anything"
    );
    let partial = attempt.written().await;
    assert_eq!(partial.len() as u64, committed);
    assert_eq!(partial, expected[..partial.len()]);

    // Restart: a fresh instance, resuming from the part file and the checkpoint alone.
    let second = backend(&manifest, &component);
    let resumed = Attempt {
        part: attempt.part.clone(),
        cancellation: CancellationToken::new(),
        _directory: attempt._directory,
    };
    let state = resumed
        .state(&second, committed, Some(expected.len() as u64))
        .await;
    let outcome = second
        .run(state, job(url, Some(checkpoint)))
        .await
        .expect("second attempt");
    let TransferOutcome::Complete { committed, .. } = outcome else {
        panic!("the resumed transfer should have completed");
    };
    assert_eq!(committed, expected.len() as u64);
    assert_eq!(resumed.written().await, expected, "resume spliced the file");
}

/// A backend may dial only what its own manifest names — not another port on the same host.
#[tokio::test]
async fn a_target_outside_the_manifest_is_refused() {
    let component = support::component();
    let server = support::serve(payload(1024), 1024, Duration::ZERO).await;
    // The manifest allows the server's port; the URL asks for the next one.
    let manifest = support::manifest(server.address.port());
    let backend = backend(&manifest, &component);
    let attempt = Attempt::new();
    let state = attempt.state(&backend, 0, None).await;
    let failure = backend
        .probe(
            state,
            format!(
                "example+tcp://127.0.0.1:{}/file.bin",
                server.address.port().wrapping_add(1)
            ),
            None,
        )
        .await
        .expect_err("a port the manifest does not name");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.net_target_not_allowed")
    );
}

/// Loopback is refused unless the host was told to allow it, because the service's own API
/// listens there and no transfer protocol has a reason to reach it.
#[tokio::test]
async fn loopback_is_refused_without_an_explicit_allowance() {
    let component = support::component();
    let server = support::serve(payload(1024), 1024, Duration::ZERO).await;
    let manifest = support::manifest(server.address.port());
    let backend = backend(&manifest, &component);
    let attempt = Attempt::new();
    let part = rd_files::PartFile::open(attempt.part.clone(), None)
        .await
        .expect("part file");
    let state = TransferState::new(
        TransferTarget {
            part,
            committed: 0,
            total: None,
        },
        attempt.cancellation.clone(),
        rd_limits::ScopedLimiter::unlimited(),
        backend.manifest().capabilities.net_stream.clone(),
        Arc::new(rd_http::tls_client_config(&[]).expect("tls")),
        Arc::new(|_, _| {}),
    );
    let failure = backend
        .probe(
            state,
            format!("example+tcp://127.0.0.1:{}/file.bin", server.address.port()),
            None,
        )
        .await
        .expect_err("loopback without the allowance");
    assert_eq!(failure.code.as_deref(), Some("plugin.net_local_target"));
}

/// The speed limit reaches a protocol the core knows nothing about, because the pacing sits
/// in the host's read rather than in the plugin's goodwill.
#[tokio::test]
async fn the_bandwidth_limit_reaches_a_plugin_protocol() {
    let component = support::component();
    let expected = payload(120_000);
    let server = support::serve(expected.clone(), 32 * 1024, Duration::ZERO).await;
    let manifest = support::manifest(server.address.port());
    let backend = backend(&manifest, &component);
    let url = format!("example+tcp://127.0.0.1:{}/file.bin", server.address.port());

    let attempt = Attempt::new();
    let part = rd_files::PartFile::open(attempt.part.clone(), None)
        .await
        .expect("part file");
    let limiter = rd_limits::LimiterRegistry::new();
    limiter.set_manual_limit(Some(60_000));
    let state = TransferState::new(
        TransferTarget {
            part,
            committed: 0,
            total: None,
        },
        attempt.cancellation.clone(),
        limiter.scoped(rd_limits::TransferScope::default()),
        backend.manifest().capabilities.net_stream.clone(),
        Arc::new(rd_http::tls_client_config(&[]).expect("tls")),
        Arc::new(|_, _| {}),
    )
    .allowing_local_targets();
    let started = std::time::Instant::now();
    let outcome = backend.run(state, job(url, None)).await.expect("attempt");
    assert!(matches!(outcome, TransferOutcome::Complete { .. }));
    assert!(
        started.elapsed() >= Duration::from_millis(700),
        "120 KB at 60 KB/s cannot arrive in {:?}",
        started.elapsed()
    );
}

/// The host promotes only what it verified, and only into the package folder.
///
/// The runner is the half a plugin cannot reach: it opens the part file, checks the length
/// against what the probe reported, and renames. A backend that claims success on a short
/// file therefore produces a failed attempt and leaves the partial data where a retry can
/// continue from it — the file it lied about is never presented as finished.
#[tokio::test]
async fn the_host_verifies_the_length_before_it_promotes() {
    let component = support::component();
    let expected = payload(64_000);
    let server = support::serve(expected.clone(), 16 * 1024, Duration::ZERO).await;
    let manifest_toml = support::manifest(server.address.port());
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

    let manifest: PluginManifest = toml::from_str(&manifest_toml).expect("manifest");
    let backends = rd_plugin_transfer::TransferBackends::for_test(
        vec![Arc::new(
            TransferBackend::new(manifest, &component).expect("backend"),
        )],
        true,
    );
    let runner = rd_plugin_transfer::build(backends, database.clone(), Vec::new());

    let outcome = rd_scheduler::ExternalRunner::run(
        runner.as_ref(),
        &file,
        &package,
        CancellationToken::new(),
        rd_scheduler::RunLimits {
            max_parallel_requests: 1,
            bandwidth: rd_limits::ScopedLimiter::unlimited(),
        },
    )
    .await
    .expect("attempt");

    match outcome {
        rd_scheduler::RunOutcome::Completed { final_name } => {
            assert_eq!(final_name, "file.bin");
            let promoted = tokio::fs::read(destination.join("file.bin"))
                .await
                .expect("the promoted file is inside the package folder");
            assert_eq!(promoted, expected);
            // The staging file is gone: promotion is a rename, not a copy.
            assert!(
                !destination
                    .join(".rdownloader")
                    .join(format!("{}.part", file.id))
                    .exists()
            );
        }
        other => panic!("expected a completed transfer, got {other:?}"),
    }

    // A finished transfer keeps no resume state around.
    assert!(
        database
            .plugin_transfer(file.id)
            .await
            .expect("read checkpoint")
            .is_none()
    );
}

/// A job that started on one backend version is never handed to another.
///
/// The checkpoint is that version's private format, so continuing with a different build
/// would resume from an offset only the first one could interpret. The job fails loudly
/// instead, which is recoverable, rather than writing plausible-looking rubbish.
#[tokio::test]
async fn a_running_job_is_not_moved_to_another_backend_version() {
    let component = support::component();
    let server = support::serve(payload(4_096), 4_096, Duration::ZERO).await;
    let manifest_toml = support::manifest(server.address.port());
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

    // The job is mid-transfer on 0.7.0 …
    let stored: PluginManifest = toml::from_str(&manifest_toml).expect("manifest");
    database
        .save_plugin_transfer(
            file.id,
            stored.id.to_string(),
            "0.7.0".to_owned(),
            Some(vec![0, 0, 0, 0, 0, 0, 4, 0]),
        )
        .await
        .expect("checkpoint");

    // … and an upgrade replaced it with 0.8.0.
    let upgraded: PluginManifest =
        toml::from_str(&manifest_toml.replace(r#"version = "0.7.0""#, r#"version = "0.8.0""#))
            .expect("manifest");
    let backends = rd_plugin_transfer::TransferBackends::for_test(
        vec![Arc::new(
            TransferBackend::new(upgraded, &component).expect("backend"),
        )],
        true,
    );
    let runner = rd_plugin_transfer::build(backends, database.clone(), Vec::new());

    let outcome = rd_scheduler::ExternalRunner::run(
        runner.as_ref(),
        &file,
        &package,
        CancellationToken::new(),
        rd_scheduler::RunLimits {
            max_parallel_requests: 1,
            bandwidth: rd_limits::ScopedLimiter::unlimited(),
        },
    )
    .await
    .expect("attempt");

    match outcome {
        rd_scheduler::RunOutcome::Failed(failure) => assert_eq!(
            failure.code.as_deref(),
            Some("plugin.pinned_version_missing")
        ),
        other => panic!("the pinned version is gone; expected a failure, got {other:?}"),
    }
}
