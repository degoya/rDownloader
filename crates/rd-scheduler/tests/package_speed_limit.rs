//! A package's own speed limit (RD-1100-01) as the scheduler applies it: read from the database
//! on every reload, the binding limit of that package's files and of no other, and still there
//! after a change of the global limits and after a restart.

use std::path::Path;

use rd_core::{DownloadKind, PackageId};
use rd_limits::{LimitSource, TransferScope};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn start(directory: &Path, database: &rd_db::Database) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            // Nothing is transferred: the test is about the limits, not about a race with a
            // worker.
            max_active_files: 0,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler")
}

async fn package(scheduler: &SchedulerHandle, directory: &Path) -> PackageId {
    let spec = PackageSpec {
        name: format!("package {}", PackageId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: "https://example.invalid/file.bin".parse().expect("url"),
        file_name: "file.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind: DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }];
    let (package, _) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    package.id
}

fn file_of(package: PackageId) -> TransferScope {
    TransferScope::for_download(DownloadKind::Http, Some("example.invalid"), None, None)
        .in_package(package)
}

fn binding(scheduler: &SchedulerHandle, package: PackageId) -> Option<(LimitSource, u64)> {
    scheduler
        .bandwidth()
        .binding_limit(&file_of(package))
        .map(|limit| (limit.source, limit.bytes_per_second))
}

#[tokio::test]
async fn a_package_limit_binds_its_own_files_and_survives_a_global_change_and_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("package-limit.sqlite3");
    let database = rd_db::Database::open(&path).await.expect("database");
    let scheduler = start(directory.path(), &database).await;
    let limited = package(&scheduler, directory.path()).await;
    let other = package(&scheduler, directory.path()).await;

    database
        .set_package_speed_limit(limited, Some(100_000))
        .await
        .expect("limit");
    scheduler.reload_bandwidth().await.expect("reload");
    assert_eq!(
        binding(&scheduler, limited),
        Some((LimitSource::Package, 100_000))
    );
    assert_eq!(binding(&scheduler, other), None);

    // A stricter hand-set limit wins while it holds; the package's own one is still there
    // once it is lifted, and a reload (the profile switch path) does not take it either.
    let limits = scheduler.bandwidth().limits();
    limits.set_manual_limit(Some(50_000));
    assert_eq!(
        binding(&scheduler, limited),
        Some((LimitSource::Manual, 50_000))
    );
    limits.set_manual_limit(Some(10_000_000));
    scheduler.reload_bandwidth().await.expect("reload");
    assert_eq!(
        binding(&scheduler, limited),
        Some((LimitSource::Package, 100_000))
    );
    assert_eq!(
        binding(&scheduler, other),
        Some((LimitSource::Manual, 10_000_000))
    );
    limits.set_manual_limit(None);

    scheduler.shutdown().await.expect("shutdown");
    database.close().await.expect("close");
    let database = rd_db::Database::open(&path).await.expect("reopen");
    let restarted = start(directory.path(), &database).await;
    restarted.reload_bandwidth().await.expect("reload");
    assert_eq!(
        binding(&restarted, limited),
        Some((LimitSource::Package, 100_000))
    );

    // Removing the limit reaches the registry with the next reload.
    database
        .set_package_speed_limit(limited, None)
        .await
        .expect("clear");
    restarted.reload_bandwidth().await.expect("reload");
    assert_eq!(binding(&restarted, limited), None);
    restarted.shutdown().await.expect("shutdown");
}
