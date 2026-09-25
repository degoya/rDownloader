//! Tests of the persistent transfer statistics (RD-110-01), kept beside the store so the
//! store itself stays under the file-length rule.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chrono::{Duration, Utc};
use rd_core::{AuthProfileSelection, DownloadId, DownloadKind, DownloadState, PackageId};

use crate::{
    Database, NewAccount, NewDownload, NewPackage,
    stats_store::{DIRECT_PROVIDER, PRUNE_BATCH, StatsResolution, StatsRetention},
};

async fn open(name: &str) -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join(name))
        .await
        .expect("database");
    (directory, database)
}

async fn queued_download(
    database: &Database,
    directory: &std::path::Path,
    kind: DownloadKind,
    account_id: Option<rd_core::AccountId>,
) -> rd_core::DownloadFile {
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "stats".to_owned(),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/secret-name.bin".parse().expect("URL"),
            file_name: "secret-name.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
}

/// Writes one hourly row straight into the table, as the sweep tests need thousands.
async fn seed_hour(database: &Database, start: &str, kind: &str, bytes: i64) {
    sqlx::query(
        "INSERT INTO transfer_stats \
         (resolution, bucket_start, kind, provider, completed, failed, retries, bytes, seconds) \
         VALUES ('hour', ?, ?, 'direct', 1, 0, 0, ?, 10)",
    )
    .bind(start)
    .bind(kind)
    .bind(bytes)
    .execute(&database.readers)
    .await
    .expect("seed");
}

/// Completion, a scheduled retry and a final failure each land in the current hour's
/// bucket and in the totals, under the transport and the account's provider — never
/// under the file name or the address.
#[tokio::test]
async fn outcomes_are_counted_by_kind_and_provider_in_bucket_and_totals() {
    let (directory, database) = open("outcomes.sqlite").await;
    let account = database
        .create_account(NewAccount {
            provider: "rapidgator".to_owned(),
            label: "Alex's account".to_owned(),
            username: Some("alex@example.test".to_owned()),
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let hosted = queued_download(
        &database,
        directory.path(),
        DownloadKind::Http,
        Some(account.id),
    )
    .await;
    database
        .set_download_progress(hosted.id, 4_096, Some(4_096))
        .await
        .expect("progress");
    for next in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
    ] {
        database
            .transition_download(hosted.id, next)
            .await
            .expect("transition");
    }
    database
        .complete_download(hosted.id, "secret-name.bin".to_owned(), None)
        .await
        .expect("complete");

    let retried = queued_download(&database, directory.path(), DownloadKind::Media, None).await;
    database
        .record_failure(
            retried.id,
            rd_core::Failure::new(
                rd_core::FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "reset",
            ),
            Some(Utc::now() + Duration::minutes(5)),
        )
        .await
        .expect("retry");
    database
        .record_failure(
            retried.id,
            rd_core::Failure::new(rd_core::FailureKind::Permanent, "reset"),
            None,
        )
        .await
        .expect("final failure");

    let totals = database.list_transfer_totals().await.expect("totals");
    let hosted_total = totals
        .iter()
        .find(|total| total.provider == "rapidgator")
        .expect("the hosted transfer's provider row");
    assert_eq!(hosted_total.kind, "http");
    assert_eq!(hosted_total.completed, 1);
    assert_eq!(hosted_total.bytes, 4_096);
    let direct_total = totals
        .iter()
        .find(|total| total.provider == DIRECT_PROVIDER)
        .expect("the direct transfer's row");
    assert_eq!(direct_total.kind, "media");
    assert_eq!(
        (
            direct_total.retries,
            direct_total.failed,
            direct_total.completed
        ),
        (1, 1, 0)
    );

    let buckets = database
        .list_transfer_stats(StatsResolution::Hour, Utc::now() - Duration::hours(1))
        .await
        .expect("buckets");
    assert_eq!(buckets.len(), 2, "{buckets:?}");
    assert!(
        buckets
            .iter()
            .all(|bucket| bucket.bucket_start == StatsResolution::Hour.bucket_start(Utc::now()))
    );
    let rendered = format!("{buckets:?}{totals:?}");
    for forbidden in ["secret-name", "example.test", "Alex", "alex@"] {
        assert!(
            !rendered.contains(forbidden),
            "{forbidden} reached the statistics"
        );
    }
    let _ = directory;
}

/// Hourly rows past the hourly window are added into their day's row; rows past the
/// retention are deleted; the totals are untouched by either.
#[tokio::test]
async fn stale_hours_fold_into_days_and_expired_days_go() {
    let (_directory, database) = open("prune.sqlite").await;
    let now = Utc::now();
    let old_day = now - Duration::days(40);
    for hour in 0..24 {
        let start = StatsResolution::Hour.bucket_start(old_day - Duration::hours(hour));
        seed_hour(&database, &start, "http", 100).await;
    }
    let fresh = StatsResolution::Hour.bucket_start(now);
    seed_hour(&database, &fresh, "http", 7).await;
    let expired = StatsResolution::Day.bucket_start(now - Duration::days(400));
    sqlx::query(
        "INSERT INTO transfer_stats (resolution, bucket_start, kind, provider, completed) \
         VALUES ('day', ?, 'http', 'direct', 1)",
    )
    .bind(&expired)
    .execute(&database.readers)
    .await
    .expect("expired day");

    let report = database
        .prune_transfer_stats(StatsRetention {
            hourly_days: 30,
            retention_days: 365,
        })
        .await
        .expect("prune");
    assert_eq!(report.downsampled, 24);
    assert_eq!(report.deleted, 1);
    assert!(!report.more);

    let days = database
        .list_transfer_stats(StatsResolution::Day, now - Duration::days(60))
        .await
        .expect("days");
    let folded: i64 = days.iter().map(|bucket| bucket.bytes).sum();
    let completed: i64 = days.iter().map(|bucket| bucket.completed).sum();
    assert_eq!((folded, completed), (2_400, 24), "{days:?}");
    assert!(
        days.len() <= 2,
        "24 hours fold into at most two calendar days: {days:?}"
    );
    let hours = database
        .list_transfer_stats(StatsResolution::Hour, now - Duration::days(60))
        .await
        .expect("hours");
    assert_eq!(hours.len(), 1, "only the fresh hour survives: {hours:?}");
    assert_eq!(hours[0].bytes, 7);
    let expired_left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transfer_stats WHERE bucket_start = ?")
            .bind(&expired)
            .fetch_one(&database.readers)
            .await
            .expect("count");
    assert_eq!(expired_left, 0);
}

/// The sweep never holds the writer for more than one batch, so a queue write issued
/// while thousands of rows are being thinned is answered before the sweep is over.
#[tokio::test]
async fn a_sweep_over_thousands_of_rows_does_not_block_a_queue_write() {
    let (directory, database) = open("sweep.sqlite").await;
    let now = Utc::now();
    let rows = PRUNE_BATCH * 6;
    for index in 0..rows {
        let start = StatsResolution::Hour
            .bucket_start(now - Duration::days(400) - Duration::hours(index.into()));
        seed_hour(&database, &start, "http", 1).await;
    }
    let retention = StatsRetention {
        hourly_days: 30,
        retention_days: 365,
    };
    let batches = Arc::new(AtomicUsize::new(0));
    let sweep = {
        let database = database.clone();
        let batches = Arc::clone(&batches);
        tokio::spawn(async move {
            loop {
                let report = database
                    .prune_transfer_stats(retention)
                    .await
                    .expect("prune");
                assert!(
                    report.downsampled + report.deleted <= u64::from(PRUNE_BATCH),
                    "a pass touched {report:?}"
                );
                batches.fetch_add(1, Ordering::SeqCst);
                if !report.more {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
    };
    let download = queued_download(&database, directory.path(), DownloadKind::Http, None).await;
    let seen = batches.load(Ordering::SeqCst);
    let total = sweep
        .await
        .map(|()| batches.load(Ordering::SeqCst))
        .expect("sweep");
    assert!(
        total >= 6,
        "{rows} rows need at least six passes, got {total}"
    );
    assert!(
        seen < total,
        "the queue write waited for the whole sweep ({seen} of {total})"
    );
    assert_eq!(download.state, DownloadState::Queued);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfer_stats")
        .fetch_one(&database.readers)
        .await
        .expect("count");
    assert_eq!(left, 0, "everything past the retention is gone");
}

/// The bucket key is fixed-width UTC, so string order is time order.
#[test]
fn bucket_starts_are_fixed_width_and_ordered() {
    let moment = "2026-09-20T14:35:12Z".parse().expect("moment");
    assert_eq!(
        StatsResolution::Hour.bucket_start(moment),
        "2026-09-20T14:00:00Z"
    );
    assert_eq!(
        StatsResolution::Day.bucket_start(moment),
        "2026-09-20T00:00:00Z"
    );
    let later = "2026-10-01T00:00:00Z".parse().expect("moment");
    assert!(StatsResolution::Hour.bucket_start(moment) < StatsResolution::Hour.bucket_start(later));
}
