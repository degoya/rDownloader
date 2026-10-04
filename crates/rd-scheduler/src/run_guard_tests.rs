//! Audit 1.9.1 — what follows an attempt whatever the attempt did (TR-03, TR-05, TR-06, RA-TR-01).
//!
//! Against a real database and the real dispatcher, with a stand-in runner: the slot, the row's
//! state and the mirror handover are what the cases look at, and all three are written by the
//! scheduler, not by the runner.

use std::{
    path::Path,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use rd_core::{DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure, FailureKind};
use tokio_util::sync::CancellationToken;

use crate::{
    ExternalRunner, FileSpec, PackageSpec, RunLimits, RunOutcome, SchedulerConfig, SchedulerHandle,
};

/// What the stand-in FTP runner does with a file.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Panics inside the attempt.
    Panic,
    /// Fails the first host's file as offline and holds every other one until cancelled.
    FailFirstHost,
    /// Returns an `Err` for the first host's file, the way a missing tool does, and holds every
    /// other one until cancelled.
    ErrFirstHost,
}

struct StandIn(Behaviour);

#[async_trait::async_trait]
impl ExternalRunner for StandIn {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Ftp
    }

    fn slot_capacity(&self) -> usize {
        4
    }

    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability::default()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        _package: &DownloadPackage,
        cancellation: CancellationToken,
        _limits: RunLimits,
    ) -> anyhow::Result<RunOutcome> {
        match self.0 {
            Behaviour::Panic => panic!("a runner that panics mid-transfer"),
            Behaviour::FailFirstHost if file.source.host_str() == Some("first.example") => Ok(
                RunOutcome::Failed(Failure::new(FailureKind::Offline, "no route to host")),
            ),
            Behaviour::ErrFirstHost if file.source.host_str() == Some("first.example") => {
                Err(anyhow::anyhow!("the tool is not installed"))
            }
            Behaviour::FailFirstHost | Behaviour::ErrFirstHost => {
                cancellation.cancelled().await;
                Ok(RunOutcome::Stopped)
            }
        }
    }
}

async fn scheduler_with(
    directory: &Path,
    behaviour: Behaviour,
) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("guard.sqlite3"))
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
        vec![Arc::new(StandIn(behaviour))],
    )
    .await
    .expect("scheduler");
    (scheduler, database)
}

fn package(directory: &Path, start_paused: bool) -> PackageSpec {
    PackageSpec {
        name: "release".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    }
}

fn ftp_file(source: &str, mirror_group: Option<&str>, skipped: bool) -> FileSpec {
    FileSpec {
        source: source.parse().expect("url"),
        file_name: "release.mkv".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind: DownloadKind::Ftp,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: mirror_group.map(str::to_owned),
        skipped,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// Polls `database` until `done` holds for the row, or fails after ten seconds.
async fn until(
    database: &rd_db::Database,
    file: &DownloadFile,
    done: impl Fn(&DownloadFile) -> bool,
) -> DownloadFile {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = database
            .get_download(file.id)
            .await
            .expect("read")
            .expect("download still there");
        if done(&current) {
            return current;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the row never got there; it is {:?} with {:?}",
            current.state,
            current.last_error
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// TR-05: a panic in a runner ended the task that owned the slot, and nothing gave it back.
#[tokio::test]
async fn a_panicking_runner_gives_its_slot_back_and_leaves_a_failed_attempt() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_with(directory.path(), Behaviour::Panic).await;
    // No retry, so the row stays where the guard put it instead of panicking again.
    scheduler.max_retries.store(0, Ordering::Release);

    let (_, files) = scheduler
        .enqueue_package(
            package(directory.path(), false),
            vec![ftp_file("ftp://first.example/release.mkv", None, false)],
        )
        .await
        .expect("enqueue");

    let failed = until(&database, &files[0], |file| {
        file.state == DownloadState::Failed
    })
    .await;
    assert!(
        failed
            .last_error
            .as_ref()
            .is_some_and(|failure| failure.message.contains("panicked")),
        "the panic is the recorded reason: {:?}",
        failed.last_error
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while scheduler
        .active
        .lock()
        .await
        .tokens
        .contains_key(&files[0].id)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the slot of the panicked attempt was never given back"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        scheduler
            .runners
            .try_slot(DownloadKind::Ftp, 0)
            .await
            .is_some(),
        "the kind's place came back with the task"
    );
}

/// TR-06: a pass that runs after the shutdown collected the tokens must not start anything.
#[tokio::test]
async fn a_dispatch_pass_after_the_shutdown_starts_nothing() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_with(directory.path(), Behaviour::FailFirstHost).await;
    scheduler.shutdown().await.expect("shutdown");

    let (_, files) = scheduler
        .enqueue_package(
            package(directory.path(), false),
            vec![ftp_file("ftp://second.example/release.mkv", None, false)],
        )
        .await
        .expect("enqueue");
    scheduler.schedule_runnable().await.expect("pass");

    assert!(
        scheduler.active.lock().await.tokens.is_empty(),
        "a token nobody will cancel was handed out after the shutdown"
    );
    let row = database
        .get_download(files[0].id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.state, DownloadState::Queued);
}

/// TR-03: a failed FTP mirror went straight to `record_failure` and never handed the turn on,
/// so the group stood still until the next start's sweep.
#[tokio::test]
async fn a_failed_ftp_mirror_hands_the_turn_to_the_next_one() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_with(directory.path(), Behaviour::FailFirstHost).await;

    let (_, files) = scheduler
        .enqueue_package(
            package(directory.path(), false),
            vec![
                ftp_file("ftp://first.example/release.mkv", Some("release"), false),
                ftp_file("ftp://second.example/release.mkv", Some("release"), true),
            ],
        )
        .await
        .expect("enqueue");

    let given_up = until(&database, &files[0], |file| {
        file.state == DownloadState::Failed
    })
    .await;
    assert_eq!(
        given_up
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some(crate::mirrors::HANDOVER_CODE),
        "the FTP member's failure goes the same way as an HTTP member's"
    );
    until(&database, &files[1], |file| {
        matches!(
            file.state,
            DownloadState::Queued | DownloadState::Resolving | DownloadState::Downloading
        )
    })
    .await;
    // The waiting mirror is held by the stand-in until cancelled; let it go.
    scheduler.shutdown().await.expect("shutdown");
}

/// RA-TR-01: a runner that returned an `Err` (a missing `yt-dlp`, say) went to
/// `record_failure` directly, so a mirror group out of attempts left its waiting members
/// `Skipped` until the next start.
#[tokio::test]
async fn a_runner_error_on_a_mirror_hands_the_turn_to_the_next_one() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_with(directory.path(), Behaviour::ErrFirstHost).await;
    // No retry, so the first error is the last attempt and the group has to hand over.
    scheduler.max_retries.store(0, Ordering::Release);

    let (_, files) = scheduler
        .enqueue_package(
            package(directory.path(), false),
            vec![
                ftp_file("ftp://first.example/release.mkv", Some("release"), false),
                ftp_file("ftp://second.example/release.mkv", Some("release"), true),
            ],
        )
        .await
        .expect("enqueue");

    let given_up = until(&database, &files[0], |file| {
        file.state == DownloadState::Failed
    })
    .await;
    assert_eq!(
        given_up
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some(crate::mirrors::HANDOVER_CODE),
        "the runner's error goes the same way as a failure it reported"
    );
    until(&database, &files[1], |file| {
        matches!(
            file.state,
            DownloadState::Queued | DownloadState::Resolving | DownloadState::Downloading
        )
    })
    .await;
    scheduler.shutdown().await.expect("shutdown");
}
