//! RD-1291-02: the traffic budget counts from the first sample, also when a profile has been
//! active at every sample since the installation, and charges no profile for earlier traffic.
//!
//! Against a real database, with the supervisor stopped, so every sample is the test's own
//! `supervise_bandwidth` — a reload from the database and a count, as the fifteen-second tick
//! does.

use std::path::Path;

use rd_core::{DownloadId, DownloadKind};
use rd_limits::ManualEnd;

use super::BUDGET_BASELINE_KEY;
use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("budget.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    scheduler.shutdown().await.expect("shutdown");
    (scheduler, database)
}

/// One paused file whose committed bytes stand in for the queue's traffic.
async fn file(scheduler: &SchedulerHandle, directory: &Path) -> DownloadId {
    let spec = PackageSpec {
        name: "traffic".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let file = FileSpec {
        source: "https://files.example/traffic.bin".parse().expect("url"),
        file_name: "traffic.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind: DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    };
    let (_, files) = scheduler
        .enqueue_package(spec, vec![file])
        .await
        .expect("enqueue");
    files[0].id
}

/// Switches on, by hand, a profile with a daily budget of `limit` bytes.
async fn capped(scheduler: &SchedulerHandle, database: &rd_db::Database, limit: u64) {
    let profile = database
        .create_bandwidth_profile(rd_db::NewBandwidthProfile {
            name: "Tiny".to_owned(),
            download_bytes_per_second: None,
            upload_bytes_per_second: None,
            max_active_files: None,
            daily_budget_bytes: Some(rd_core::ByteCount::new(limit).expect("limit")),
            monthly_budget_bytes: None,
            scopes: Vec::new(),
            pause_downloads: false,
        })
        .await
        .expect("profile");
    scheduler
        .switch_bandwidth_profile(Some(profile.id), ManualEnd::Never, None)
        .await
        .expect("switch");
}

/// The queue has committed `bytes` in all, then one supervision cycle samples it.
async fn sample(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    id: DownloadId,
    bytes: u64,
) {
    database
        .set_download_progress(id, bytes, None)
        .await
        .expect("progress");
    scheduler.supervise_bandwidth().await.expect("supervise");
}

/// Today's traffic of the active profile, as the status reports it.
async fn used_today(scheduler: &SchedulerHandle) -> u64 {
    scheduler
        .bandwidth()
        .status()
        .await
        .budget
        .map_or(0, |budget| budget.daily.used_bytes)
}

/// The tester's case: the profile is switched on before the first sample, so no sample ever ran
/// without one. 1.29.0 started every sample at the current total and never counted.
#[tokio::test]
async fn a_profile_active_from_the_first_sample_counts_and_runs_out_once() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let id = file(&scheduler, directory.path()).await;
    capped(&scheduler, &database, 1000).await;
    let mut events = database.subscribe();

    // Traffic from before the first sample is nobody's: the sample only sets the odometer.
    sample(&scheduler, &database, id, 300).await;
    assert_eq!(
        database
            .get_setting(BUDGET_BASELINE_KEY)
            .await
            .expect("setting"),
        Some(serde_json::json!(300)),
        "the first sample stores the odometer"
    );
    assert_eq!(used_today(&scheduler).await, 0);

    sample(&scheduler, &database, id, 800).await;
    assert_eq!(used_today(&scheduler).await, 500, "the budget counts");
    assert!(scheduler.bandwidth().budget_exceeded().await.is_none());

    sample(&scheduler, &database, id, 1500).await;
    assert_eq!(used_today(&scheduler).await, 1200);
    let exceeded = scheduler
        .bandwidth()
        .budget_exceeded()
        .await
        .expect("the budget ran out");
    assert_eq!(exceeded.limit_bytes, 1000);
    // Still used up on the next samples, with or without traffic: nothing new to announce.
    sample(&scheduler, &database, id, 1500).await;
    sample(&scheduler, &database, id, 1700).await;
    assert!(scheduler.bandwidth().budget_exceeded().await.is_some());

    let mut announced = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == rd_core::EventKind::BandwidthChanged
            && event.payload["entity"] == "budget"
            && event.payload["exhausted"] == true
        {
            announced += 1;
        }
    }
    assert_eq!(announced, 1, "budget_exhausted goes out once");
}

/// A profile counting for the first time after traffic without one is not charged for it.
#[tokio::test]
async fn a_profile_counting_for_the_first_time_is_not_charged_for_earlier_traffic() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let id = file(&scheduler, directory.path()).await;

    sample(&scheduler, &database, id, 5000).await;
    capped(&scheduler, &database, 1000).await;
    sample(&scheduler, &database, id, 5000).await;
    assert_eq!(used_today(&scheduler).await, 0, "no back-charge");
    assert!(scheduler.bandwidth().budget_exceeded().await.is_none());

    sample(&scheduler, &database, id, 5400).await;
    assert_eq!(used_today(&scheduler).await, 400);
}
