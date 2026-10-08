//! RD-1190-13/-14 — an account whose traffic is used up: its files wait instead of being
//! blocked, the setting decides what else waits, and a check that finds traffic again lets them
//! go without lifting a pause somebody set.
//!
//! Against a real database and a scheduler that admits no transfer (`max_active_files: 0`), so
//! every state below is this module's doing, not a worker's.

use std::{collections::BTreeMap, path::Path};

use chrono::{Duration, Utc};
use rd_core::{AccountId, AccountTrafficAction, DownloadFile, DownloadState, Failure, FailureKind};

use super::{TRAFFIC_CHECK_INTERVAL_MINUTES, TrafficHolds};
use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn start(directory: &Path, database: &rd_db::Database) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
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

async fn account(database: &rd_db::Database) -> AccountId {
    database
        .create_account(rd_db::NewAccount {
            provider: "ddownload".to_owned(),
            label: format!("traffic {}", AccountId::new()),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

/// One package of one paused file through `account`.
async fn file(scheduler: &SchedulerHandle, directory: &Path, account: AccountId) -> DownloadFile {
    let spec = PackageSpec {
        name: format!("package {}", rd_core::DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: "https://ddownload.com/abc123xyz/part1.rar"
            .parse()
            .expect("url"),
        file_name: "part1.rar".to_owned(),
        size: None,
        account_id: Some(account),
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }];
    let (_, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    files.into_iter().next().expect("one file")
}

fn traffic_exhausted() -> Failure {
    Failure::coded(
        FailureKind::RateLimited {
            retry_after_seconds: Some(3600),
        },
        "ddownload.traffic_exhausted",
        "DDownload account traffic is used up",
    )
}

async fn row(database: &rd_db::Database, id: rd_core::DownloadId) -> DownloadFile {
    database.get_download(id).await.expect("read").expect("row")
}

fn configure(scheduler: &SchedulerHandle, action: AccountTrafficAction) {
    scheduler.traffic_holds.configure(action, BTreeMap::new());
}

#[test]
fn each_action_holds_what_it_says_and_an_override_beats_the_default() {
    let now = Utc::now();
    let (held, other) = (AccountId::new(), AccountId::new());
    let holds = TrafficHolds::default();
    holds.hold(held, now + Duration::hours(1), now);

    holds.configure(AccountTrafficAction::Nothing, BTreeMap::new());
    assert!(!holds.holds_back(held, now), "nothing holds no other file");
    assert_eq!(holds.settle(now), None, "nothing holds no queue");

    holds.configure(AccountTrafficAction::PauseAccount, BTreeMap::new());
    assert!(holds.holds_back(held, now));
    assert!(
        !holds.holds_back(other, now),
        "another account keeps downloading"
    );
    assert_eq!(holds.settle(now), None);

    holds.configure(AccountTrafficAction::PauseQueue, BTreeMap::new());
    assert!(
        !holds.holds_back(held, now),
        "the queue's hold covers it instead"
    );
    assert_eq!(holds.settle(now), Some(true));
    assert_eq!(holds.settle(now), None, "written once, on the change");

    holds.configure(
        AccountTrafficAction::PauseQueue,
        BTreeMap::from([(held, AccountTrafficAction::Nothing)]),
    );
    assert_eq!(holds.action_for(held), AccountTrafficAction::Nothing);
    assert_eq!(holds.action_for(other), AccountTrafficAction::PauseQueue);
    assert_eq!(
        holds.settle(now),
        Some(false),
        "the override released the queue"
    );
}

#[test]
fn a_hold_ends_with_the_hosters_wait_and_is_checked_every_interval_before() {
    let now = Utc::now();
    let account = AccountId::new();
    let holds = TrafficHolds::default();
    holds.configure(AccountTrafficAction::PauseQueue, BTreeMap::new());
    holds.hold(account, now + Duration::hours(1), now);
    assert_eq!(holds.settle(now), Some(true));

    assert!(
        holds.due_checks(now).is_empty(),
        "the first check is an interval away"
    );
    let check = now + Duration::minutes(TRAFFIC_CHECK_INTERVAL_MINUTES);
    assert_eq!(holds.due_checks(check), vec![account]);
    assert!(holds.due_checks(check).is_empty(), "moved one interval on");

    let after = now + Duration::hours(1);
    assert!(!holds.holds_back(account, after));
    assert_eq!(
        holds.settle(after),
        Some(false),
        "the wait ended, so does the hold"
    );
    assert!(holds.list(after).is_empty());
}

#[tokio::test]
async fn used_up_traffic_waits_instead_of_blocking_and_holds_the_accounts_other_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("traffic.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    let (limited, other) = (account(&database).await, account(&database).await);
    let first = file(&scheduler, directory.path(), limited).await;
    let second = file(&scheduler, directory.path(), limited).await;
    let elsewhere = file(&scheduler, directory.path(), other).await;

    crate::failures::record_error(&scheduler, &first, traffic_exhausted())
        .await
        .expect("record");

    let waiting = row(&database, first.id).await;
    assert_eq!(
        waiting.state,
        DownloadState::RetryWait,
        "waits, is not blocked"
    );
    let due = waiting.next_retry_at.expect("a next attempt");
    assert!((59..=60).contains(&(due - Utc::now()).num_minutes()));
    assert_eq!(waiting.retry_count, 0, "a limit spends no attempt");
    let now = Utc::now();
    assert!(scheduler.held_for_account_traffic(&second, now));
    assert!(!scheduler.held_for_account_traffic(&elsewhere, now));
    let holds = scheduler.account_traffic();
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].account_id, limited);
    assert_eq!(holds[0].action, AccountTrafficAction::PauseAccount);
    assert!(
        (holds[0].until - due).num_seconds().abs() <= 1,
        "the hold ends with the file's wait"
    );
    assert_eq!(
        scheduler.network_hold().await,
        None,
        "the queue itself runs"
    );
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_check_with_traffic_queues_the_waiting_files_and_leaves_a_paused_one_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("traffic.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    let limited = account(&database).await;
    let waiting = file(&scheduler, directory.path(), limited).await;
    let paused = file(&scheduler, directory.path(), limited).await;
    crate::failures::record_error(&scheduler, &waiting, traffic_exhausted())
        .await
        .expect("record");

    assert_eq!(
        scheduler
            .account_checked(limited, Some(0))
            .await
            .expect("check"),
        0,
        "no traffic, no change"
    );
    assert_eq!(
        scheduler
            .account_checked(limited, None)
            .await
            .expect("check"),
        0,
        "an unknown figure is no traffic"
    );
    assert_eq!(
        row(&database, waiting.id).await.state,
        DownloadState::RetryWait
    );

    assert_eq!(
        scheduler
            .account_checked(limited, Some(1 << 30))
            .await
            .expect("check"),
        1
    );
    assert_eq!(
        row(&database, waiting.id).await.state,
        DownloadState::Queued
    );
    assert_eq!(
        row(&database, paused.id).await.state,
        DownloadState::Paused,
        "a file somebody paused stays paused"
    );
    assert!(scheduler.account_traffic().is_empty());
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pausing_the_queue_for_traffic_never_lifts_a_pause_somebody_set() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("traffic.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    configure(&scheduler, AccountTrafficAction::PauseQueue);
    let limited = account(&database).await;
    let waiting = file(&scheduler, directory.path(), limited).await;

    crate::failures::record_error(&scheduler, &waiting, traffic_exhausted())
        .await
        .expect("record");
    assert_eq!(
        scheduler.network_hold().await,
        Some("account_traffic_exhausted")
    );

    scheduler
        .pause_queue_until(Utc::now() + Duration::hours(2))
        .await
        .expect("pause");
    scheduler
        .account_checked(limited, Some(1))
        .await
        .expect("check");
    assert_eq!(
        scheduler.network_hold().await,
        Some("queue_paused"),
        "the traffic hold went, the hand-set pause stays"
    );
    assert!(scheduler.queue_pause().await.is_some());

    scheduler.resume_queue().await.expect("resume");
    assert_eq!(scheduler.network_hold().await, None);
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_start_rebuilds_the_hold_from_the_files_still_waiting() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("traffic.sqlite3"))
        .await
        .expect("database");
    let scheduler = start(directory.path(), &database).await;
    let limited = account(&database).await;
    let waiting = file(&scheduler, directory.path(), limited).await;
    crate::failures::record_error(&scheduler, &waiting, traffic_exhausted())
        .await
        .expect("record");
    scheduler.shutdown().await.expect("shutdown");

    let restarted = start(directory.path(), &database).await;
    let holds = restarted.account_traffic();
    assert_eq!(holds.len(), 1, "{holds:?}");
    assert_eq!(holds[0].account_id, limited);
    restarted.shutdown().await.expect("shutdown");
}
