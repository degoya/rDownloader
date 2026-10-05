//! A package's own limit (RD-1100-01): the narrowest bucket, untouched by a profile switch, and
//! a measured pace for the package while another package runs at full speed.

use std::time::Instant;

use rd_core::{DownloadKind, PackageId};

use super::LimiterRegistry;
use crate::scope::{LimitScope, LimitSource, TransferScope};

fn file_of(package: PackageId) -> TransferScope {
    TransferScope::for_download(DownloadKind::Http, Some("example.com"), None, None)
        .in_package(package)
}

#[test]
fn the_narrowest_limit_wins_whether_it_is_the_package_or_a_broader_one() {
    let registry = LimiterRegistry::new();
    let package = PackageId::new();
    registry.apply(
        Some(1_000_000),
        &[(LimitScope::host("example.com"), 500_000)],
    );
    registry.set_package_limits(&[(package, 100_000)]);
    let binding = registry
        .binding_limit(&file_of(package))
        .expect("a limit applies");
    assert_eq!(binding.source, LimitSource::Package);
    assert_eq!(binding.bytes_per_second, 100_000);

    // A stricter hand-set limit binds the package too: its own limit is a ceiling, never a
    // way around the global one.
    registry.set_manual_limit(Some(50_000));
    let binding = registry
        .binding_limit(&file_of(package))
        .expect("a limit applies");
    assert_eq!(binding.source, LimitSource::Manual);
    assert_eq!(binding.bytes_per_second, 50_000);
}

#[test]
fn another_package_keeps_the_global_limit() {
    let registry = LimiterRegistry::new();
    let limited = PackageId::new();
    registry.apply(Some(1_000_000), &[]);
    registry.set_package_limits(&[(limited, 100_000)]);
    let binding = registry
        .binding_limit(&file_of(PackageId::new()))
        .expect("the global limit applies");
    assert_eq!(binding.source, LimitSource::Global);
    assert_eq!(binding.bytes_per_second, 1_000_000);
    // A transfer outside any package (a scope without one) is not touched either.
    assert_eq!(
        registry
            .binding_limit(&TransferScope::default())
            .map(|limit| limit.source),
        Some(LimitSource::Global)
    );
}

#[test]
fn a_profile_switch_leaves_the_package_limit_in_place() {
    let registry = LimiterRegistry::new();
    let package = PackageId::new();
    registry.set_package_limits(&[(package, 100_000)]);
    registry.apply(Some(5_000_000), &[]);
    registry.apply(None, &[]);
    registry.set_manual_limit(None);
    let binding = registry
        .binding_limit(&file_of(package))
        .expect("the package limit survives");
    assert_eq!(binding.source, LimitSource::Package);
    assert_eq!(binding.bytes_per_second, 100_000);
}

#[test]
fn a_package_left_out_of_the_set_loses_its_limit_and_a_changed_one_takes_the_new_rate() {
    let registry = LimiterRegistry::new();
    let (kept, dropped) = (PackageId::new(), PackageId::new());
    registry.set_package_limits(&[(kept, 100_000), (dropped, 200_000)]);
    registry.set_package_limits(&[(kept, 300_000)]);
    assert_eq!(
        registry
            .binding_limit(&file_of(kept))
            .map(|limit| limit.bytes_per_second),
        Some(300_000)
    );
    assert!(registry.binding_limit(&file_of(dropped)).is_none());
}

#[tokio::test]
async fn a_limited_package_is_paced_while_another_runs_at_full_speed() {
    const RATE: u64 = 400_000;
    const TOTAL: u64 = 1_000_000;
    const SLICE: usize = 50_000;
    let registry = LimiterRegistry::new();
    let (limited, free) = (PackageId::new(), PackageId::new());
    registry.set_package_limits(&[(limited, RATE)]);

    let slow = registry.scoped(file_of(limited));
    let fast = registry.scoped(file_of(free));
    let started = Instant::now();
    let slow_run = tokio::spawn(async move {
        let mut sent = 0;
        while sent < TOTAL {
            slow.acquire(SLICE).await.expect("acquire");
            sent += SLICE as u64;
        }
        Instant::now()
    });
    let mut sent = 0;
    while sent < TOTAL {
        fast.acquire(SLICE).await.expect("acquire");
        sent += SLICE as u64;
    }
    let fast_elapsed = started.elapsed().as_secs_f64();
    let slow_elapsed = slow_run
        .await
        .expect("slow task")
        .duration_since(started)
        .as_secs_f64();

    // The bucket holds one second's worth, so everything past the first `RATE` bytes is paced:
    // 600 000 bytes at 400 000 per second is 1.5 s at the least.
    let paced = (TOTAL - RATE) as f64 / RATE as f64;
    assert!(
        slow_elapsed >= paced * 0.95,
        "{slow_elapsed:.2} s for {TOTAL} bytes"
    );
    let rate = (TOTAL - RATE) as f64 / slow_elapsed;
    assert!(rate <= RATE as f64 * 1.05, "measured {rate:.0} B/s");
    // The other package passes no bucket at all.
    assert!(
        fast_elapsed < paced / 2.0,
        "the unlimited package took {fast_elapsed:.2} s"
    );
}
