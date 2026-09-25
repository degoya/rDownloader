//! The poll loop against a clock the test moves (RD-130-19).
//!
//! A schedule is only worth anything if the service keeps it by itself, and the only honest
//! test of that is the real loop over the real database with time under the test's control:
//! waiting for six in the morning is not a test.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use chrono::{DateTime, Local, TimeZone, Utc};
use rd_core::{SubscriptionItemState, SubscriptionKind, SubscriptionSettings};
use rd_subscription::{DiscoveredItem, PollOutcome, SourceAdapter};

use super::SubscriptionService;

/// A script adapter that counts its runs and fails when told to.
#[derive(Default)]
struct CountingScript {
    runs: AtomicUsize,
    fail: AtomicBool,
}

#[async_trait::async_trait]
impl SourceAdapter for CountingScript {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Script
    }

    async fn poll(&self, _subscription: &rd_core::Subscription) -> anyhow::Result<PollOutcome> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("script exited with status 3: login refused");
        }
        Ok(PollOutcome {
            items: vec![DiscoveredItem::new(
                "Show.S01E01.rar".to_owned(),
                "https://example.test/abc/Show.S01E01.rar".parse()?,
            )],
            ..PollOutcome::default()
        })
    }
}

fn local(day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    // January, far from any change to or from summer time in any zone that has one.
    Local
        .with_ymd_and_hms(2030, 1, day, hour, minute, second)
        .earliest()
        .expect("local time")
        .with_timezone(&Utc)
}

struct Fixture {
    service: SubscriptionService,
    database: rd_db::Database,
    script: Arc<CountingScript>,
    time: Arc<Mutex<DateTime<Utc>>>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn set(&self, time: DateTime<Utc>) {
        *self.time.lock().expect("clock") = time;
    }

    async fn tick(&self) {
        self.service
            .tick(&SubscriptionSettings::default())
            .await
            .expect("tick");
    }

    fn runs(&self) -> usize {
        self.script.runs.load(Ordering::SeqCst)
    }
}

async fn fixture(start: DateTime<Utc>) -> Fixture {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("poller.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.path().join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let media_settings = rd_media::shared_settings(&database)
        .await
        .expect("media settings");
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery settings");
    let (_media_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let remote = crate::RemoteServices::new(
        database.clone(),
        secrets.clone(),
        Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
        rd_http::SharedNetworkDefaults::default(),
    );
    let link_check = crate::link_check_service::LinkCheckService::start(
        database.clone(),
        scheduler.clone(),
        media_probe,
        remote.ftp,
        remote.sftp,
        rd_torrent::TorrentService::start(
            database.clone(),
            rd_torrent::shared_settings(&database)
                .await
                .expect("torrent settings"),
            directory.path().join("torrent"),
            directory.path().join("downloads"),
        ),
        rd_plugin_host::PluginInstaller::new(
            directory.path().join("plugins"),
            rd_plugin_host::PluginVerifier::new(true),
        ),
        scheduler.plugin_host(),
    );
    let script = Arc::new(CountingScript::default());
    let time = Arc::new(Mutex::new(start));
    let clock_time = Arc::clone(&time);
    let adapters: Vec<Arc<dyn SourceAdapter>> = vec![script.clone()];
    let service = SubscriptionService::start_with_clock(
        database.clone(),
        link_check,
        media_settings,
        gallery_settings,
        adapters,
        Arc::new(move || *clock_time.lock().expect("clock")),
    );
    Fixture {
        service,
        database,
        script,
        time,
        _directory: directory,
    }
}

fn daily_at_six() -> rd_db::NewSubscription {
    rd_db::NewSubscription {
        name: "Daily links".to_owned(),
        url: "script:daily-links.sh".parse().expect("url"),
        kind: SubscriptionKind::Script,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        // The default, which for a channel would record the first poll as history.
        backlog: rd_core::BacklogPolicy::FromNow,
        category_map: Vec::new(),
        source_categories: Vec::new(),
        every_release: false,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: Some("0 6 * * *".to_owned()),
        secret_ref: None,
    }
}

#[tokio::test]
async fn a_daily_script_runs_at_six_by_the_service_s_own_clock_and_run_now_runs_at_once() {
    let fixture = fixture(local(15, 5, 0, 0)).await;
    let created = fixture
        .database
        .create_subscription(daily_at_six())
        .await
        .expect("subscription");
    let stored = |fixture: &Fixture| {
        let database = fixture.database.clone();
        async move {
            database
                .subscription(created.id)
                .await
                .expect("get")
                .expect("exists")
        }
    };

    // Created at five: not run, but timed for six.
    fixture.tick().await;
    assert_eq!(
        fixture.runs(),
        0,
        "a scheduled subscription ran on creation"
    );
    assert_eq!(stored(&fixture).await.next_run_at, Some(local(15, 6, 0, 0)));

    fixture.set(local(15, 5, 59, 0));
    fixture.tick().await;
    assert_eq!(fixture.runs(), 0, "ran before its time");

    // Six o'clock: the loop runs it without anybody asking, and times the next day.
    fixture.set(local(15, 6, 0, 30));
    fixture.tick().await;
    assert_eq!(fixture.runs(), 1);
    let after = stored(&fixture).await;
    assert_eq!(after.next_run_at, Some(local(16, 6, 0, 0)));
    assert_eq!(after.last_error, None);
    // A script has no backlog: its first run is taken, not recorded as the past.
    let items = fixture
        .database
        .subscription_items(created.id, 10)
        .await
        .expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].state, SubscriptionItemState::Pending);

    // Nothing more until tomorrow.
    fixture.set(local(15, 12, 0, 0));
    fixture.tick().await;
    assert_eq!(fixture.runs(), 1);

    // "Run now" runs at once, whatever the schedule says, and keeps the next scheduled time.
    fixture.service.poll_now(created.id).await.expect("run now");
    assert_eq!(fixture.runs(), 2);
    assert_eq!(stored(&fixture).await.next_run_at, Some(local(16, 6, 0, 0)));

    // A failed run is in the history with its reason, and is retried before tomorrow. Half an
    // hour later, so the history -- newest first by start time -- cannot tie with the last run.
    fixture.set(local(15, 12, 30, 0));
    fixture.script.fail.store(true, Ordering::SeqCst);
    fixture.service.poll_now(created.id).await.expect("run now");
    let failed = stored(&fixture).await;
    let retry = failed.next_run_at.expect("next run");
    assert!(
        retry > local(15, 12, 30, 0) && retry < local(16, 6, 0, 0),
        "{retry}"
    );
    assert!(
        failed
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("status 3")),
        "{:?}",
        failed.last_error
    );
    let runs = fixture
        .database
        .subscription_runs(created.id, 10)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 3);
    assert!(
        runs[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("login refused")),
        "{:?}",
        runs[0].error
    );
    fixture.service.shutdown();
}

#[tokio::test]
async fn a_schedule_edit_retimes_the_subscription_instead_of_running_it() {
    let fixture = fixture(local(15, 5, 0, 0)).await;
    let created = fixture
        .database
        .create_subscription(daily_at_six())
        .await
        .expect("subscription");
    fixture.tick().await;

    let mut edit = daily_at_six();
    edit.schedule = Some("30 7 * * *".to_owned());
    fixture
        .database
        .update_subscription(created.id, edit)
        .await
        .expect("update");
    fixture.tick().await;
    assert_eq!(fixture.runs(), 0);
    let stored = fixture
        .database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.next_run_at, Some(local(15, 7, 30, 0)));
    // Retimed before the old time came: six o'clock passes without a run.
    fixture.set(local(15, 6, 0, 30));
    fixture.tick().await;
    assert_eq!(fixture.runs(), 0);
    fixture.service.shutdown();
}
