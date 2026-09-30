//! Crash and restart of the plugin transfer runner (RD-180-12, recovery matrix).
//!
//! The runner is driven twice over the same database and folder, the second time with fresh
//! backends: nothing survives from the first attempt but the part file and the rows, which is
//! what survives a killed process. Named `_contract` because it drives the reference backend
//! component, which the `no-components` profile leaves out by that suffix.

#![cfg(feature = "failpoints")]

mod support;

use std::{sync::Arc, time::Duration};

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

/// `plugin_transfer.before_checkpoint_saved`: the backend stopped with bytes on disk, and the
/// runner never recorded the checkpoint that pins them to its version.
#[tokio::test]
async fn a_transfer_stopped_before_its_checkpoint_was_saved_continues_from_the_part_file() {
    let component = support::component();
    // ~120 chunks at 5 ms each, so the stop lands well inside the transfer.
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

    // Stop once two chunks are on disk, not after a fixed time (see the contract test).
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let watched = part.clone();
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
    {
        let guard = FailpointGuard::once("plugin_transfer.before_checkpoint_saved");
        let first = runner(&manifest, &component, &database);
        let outcome = rd_scheduler::ExternalRunner::run(
            first.as_ref(),
            &file,
            &package,
            cancellation,
            limits(),
        )
        .await;
        assert!(outcome.is_err(), "the attempt ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    // What the stop left: bytes that are exactly the source's first ones, nothing recorded.
    let partial = tokio::fs::read(&part).await.expect("the part file stays");
    assert!(!partial.is_empty(), "the stop kept nothing");
    assert!(
        partial.len() < expected.len(),
        "it stopped too late to prove anything"
    );
    assert_eq!(
        partial,
        expected[..partial.len()],
        "the part file holds invented bytes"
    );
    assert!(
        database
            .plugin_transfer(file.id)
            .await
            .expect("read checkpoint")
            .is_none()
    );

    // The restart: fresh backends, the same rows and folder.
    let second = runner(&manifest, &component, &database);
    let outcome = rd_scheduler::ExternalRunner::run(
        second.as_ref(),
        &file,
        &package,
        CancellationToken::new(),
        limits(),
    )
    .await
    .expect("second attempt");
    match outcome {
        rd_scheduler::RunOutcome::Completed { final_name } => assert_eq!(final_name, "file.bin"),
        other => panic!("expected the resumed transfer to complete, got {other:?}"),
    }
    let promoted = tokio::fs::read(destination.join("file.bin"))
        .await
        .expect("the promoted file");
    assert_eq!(promoted.len(), expected.len());
    assert!(
        promoted == expected,
        "the resumed file differs from the source"
    );
    assert!(!part.exists(), "the part file survived the promotion");
    assert!(
        database
            .plugin_transfer(file.id)
            .await
            .expect("read checkpoint")
            .is_none()
    );
}
