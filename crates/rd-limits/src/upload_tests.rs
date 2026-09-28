//! The upload chain (RD-150-15): its own limits, a measured pace, a live switch.

use std::time::{Duration, Instant};

use rd_core::DownloadKind;

use super::LimiterRegistry;
use crate::scope::{LimitScope, LimitSource, TransferScope};

#[test]
fn uploads_and_downloads_do_not_share_a_bucket() {
    let registry = LimiterRegistry::new();
    registry.apply(
        Some(1_000),
        &[(LimitScope::Protocol(DownloadKind::Http), 500)],
    );
    registry.set_manual_limit(Some(2_000));
    assert!(
        registry.upload().binding_limit().is_none(),
        "a download limit must not slow an upload"
    );

    registry.apply_upload(Some(300_000));
    let binding = registry.upload().binding_limit().expect("an upload limit");
    assert_eq!(binding.bytes_per_second, 300_000);
    assert_eq!(binding.source, LimitSource::Global);
    // And the download side is where it was.
    let http = TransferScope {
        kind: Some(DownloadKind::Http),
        ..TransferScope::default()
    };
    assert_eq!(
        registry
            .binding_limit(&http)
            .map(|limit| limit.bytes_per_second),
        Some(500)
    );
}

#[test]
fn the_hand_set_upload_limit_survives_a_profile_switch() {
    let registry = LimiterRegistry::new();
    registry.set_manual_upload_limit(Some(100_000));
    registry.apply_upload(Some(400_000));
    let binding = registry.upload_binding_limit().expect("a limit applies");
    assert_eq!(binding.source, LimitSource::Manual);
    assert_eq!(binding.bytes_per_second, 100_000);

    registry.apply_upload(Some(50_000));
    assert_eq!(
        registry.upload_binding_limit().map(|limit| limit.source),
        Some(LimitSource::Global)
    );
    // A profile without an upload limit leaves the hand-set one in force.
    registry.apply_upload(None);
    assert_eq!(
        registry.upload_binding_limit().map(|limit| limit.source),
        Some(LimitSource::Manual)
    );
    registry.set_manual_upload_limit(None);
    assert!(registry.upload_binding_limit().is_none());
}

#[tokio::test]
async fn an_upload_is_paced_at_its_limit() {
    const RATE: u64 = 400_000;
    const TOTAL: u64 = 1_000_000;
    let registry = LimiterRegistry::new();
    registry.apply_upload(Some(RATE));
    let upload = registry.upload();
    let started = Instant::now();
    let mut sent = 0;
    while sent < TOTAL {
        upload.acquire(50_000).await.expect("acquire");
        sent += 50_000;
    }
    let elapsed = started.elapsed().as_secs_f64();
    // The bucket holds one second's worth, so everything past the first `RATE` bytes is paced:
    // 600 000 bytes at 400 000 per second is 1.5 s at the least.
    let paced = (TOTAL - RATE) as f64 / RATE as f64;
    assert!(elapsed >= paced * 0.95, "{elapsed:.2} s for {TOTAL} bytes");
    let rate = (TOTAL - RATE) as f64 / elapsed;
    assert!(rate <= RATE as f64 * 1.05, "measured {rate:.0} B/s");
}

#[tokio::test]
async fn a_profile_switch_reaches_an_upload_that_is_already_waiting() {
    let registry = LimiterRegistry::new();
    registry.apply_upload(Some(10_000));
    let upload = registry.upload();
    // Uses up the burst, so the next slice has to wait for the bucket.
    upload.acquire(10_000).await.expect("burst");
    let running = tokio::spawn(async move {
        // Two hundred seconds at the old rate.
        for _ in 0..200 {
            upload.acquire(10_000).await.expect("acquire");
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    registry.apply_upload(None);
    // The slice already waiting finishes under the old quota (a second at most); every later
    // one sees the new one.
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the switch reached the running upload")
        .expect("upload task");
}
