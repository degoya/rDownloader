//! RD-1240-30: what the download window holds back, what it pauses, and what its end resumes.
//!
//! The supervisor is stopped first in every test, so each pass and each supervision step is the
//! test's own. A switched-off kind makes the dispatch pass's attention visible without a
//! transfer: a file it looked at is blocked.

use std::path::Path;

use chrono::Utc;
use rd_core::{DownloadKind, DownloadState, DownloadWindow, WeeklyWindow};
use rd_limits::ManualEnd;
use tokio_util::sync::CancellationToken;

use super::ScheduleHold;
use crate::{BlockReason, FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle, StopReason};

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("window.sqlite3"))
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

fn spec(directory: &Path) -> PackageSpec {
    PackageSpec {
        name: "tonight".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    }
}

fn file(name: &str, kind: DownloadKind) -> FileSpec {
    FileSpec {
        source: format!("ftp://files.example/{name}").parse().expect("url"),
        file_name: name.to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::default(),
        kind,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// A window that is closed (`open == false`) or open right now, an hour clear of either edge,
/// in the timezone the scheduler reads windows in.
async fn window_now(scheduler: &SchedulerHandle, open: bool) -> DownloadWindow {
    let (_, timezone) = scheduler.config.bandwidth.download_pause_policy().await;
    let (_, minute) = rd_limits::local_position(timezone, Utc::now());
    let day = rd_limits::MINUTES_PER_DAY;
    let (start, end) = if open {
        ((minute + day - 60) % day, (minute + 60) % day)
    } else {
        ((minute + 60) % day, (minute + 120) % day)
    };
    DownloadWindow {
        windows: vec![WeeklyWindow {
            days: rd_limits::DaySet::EVERY_DAY.0,
            start_minute: start,
            end_minute: end,
        }],
        ignore_schedule_pause: false,
    }
}

async fn looked_at(database: &rd_db::Database, id: rd_core::DownloadId) -> bool {
    database
        .downloads_blocked_by(BlockReason::KindDisabled.as_str())
        .await
        .expect("blocked")
        .contains(&id)
}

async fn state_of(database: &rd_db::Database, id: rd_core::DownloadId) -> DownloadState {
    database
        .get_download(id)
        .await
        .expect("read")
        .expect("there")
        .state
}

/// Switches on, by hand, a profile that pauses downloads.
async fn pause_by_schedule(scheduler: &SchedulerHandle, database: &rd_db::Database) {
    let profile = database
        .create_bandwidth_profile(rd_db::NewBandwidthProfile {
            name: "Day".to_owned(),
            download_bytes_per_second: None,
            upload_bytes_per_second: None,
            max_active_files: None,
            daily_budget_bytes: None,
            monthly_budget_bytes: None,
            scopes: Vec::new(),
            pause_downloads: true,
        })
        .await
        .expect("profile");
    scheduler
        .switch_bandwidth_profile(Some(profile.id), ManualEnd::Never, None)
        .await
        .expect("switch");
    assert!(scheduler.config.bandwidth.download_pause_policy().await.0);
}

#[tokio::test]
async fn a_package_outside_its_window_is_left_out_of_the_pass() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    scheduler
        .disabled_kinds
        .lock()
        .await
        .push(DownloadKind::Ftp);
    let (package, files) = scheduler
        .enqueue_package(
            spec(directory.path()),
            vec![file("a.bin", DownloadKind::Ftp)],
        )
        .await
        .expect("enqueue");
    let closed = window_now(&scheduler, false).await;
    assert!(
        database
            .set_package_download_window(package.id, Some(closed))
            .await
            .expect("window")
    );
    scheduler.schedule_runnable().await.expect("pass");
    assert!(
        !looked_at(&database, files[0].id).await,
        "a package outside its window waits"
    );

    let open = window_now(&scheduler, true).await;
    database
        .set_package_download_window(package.id, Some(open))
        .await
        .expect("window");
    scheduler.schedule_runnable().await.expect("pass");
    assert!(
        looked_at(&database, files[0].id).await,
        "inside its window the pass takes the file up"
    );
}

#[tokio::test]
async fn the_schedule_pause_holds_new_starts_unless_the_package_ignores_it() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    scheduler
        .disabled_kinds
        .lock()
        .await
        .push(DownloadKind::Ftp);
    pause_by_schedule(&scheduler, &database).await;
    let (package, files) = scheduler
        .enqueue_package(
            spec(directory.path()),
            vec![file("a.bin", DownloadKind::Ftp)],
        )
        .await
        .expect("enqueue");
    scheduler.schedule_runnable().await.expect("pass");
    assert!(
        !looked_at(&database, files[0].id).await,
        "paused by the schedule"
    );

    // One urgent package downloads all the same.
    database
        .set_package_download_window(
            package.id,
            Some(DownloadWindow {
                windows: Vec::new(),
                ignore_schedule_pause: true,
            }),
        )
        .await
        .expect("bypass");
    scheduler.schedule_runnable().await.expect("pass");
    assert!(
        looked_at(&database, files[0].id).await,
        "the bypass lets it start"
    );
}

#[tokio::test]
async fn switching_to_no_limits_by_hand_lifts_the_schedule_pause() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    scheduler
        .disabled_kinds
        .lock()
        .await
        .push(DownloadKind::Ftp);
    pause_by_schedule(&scheduler, &database).await;
    let (_, files) = scheduler
        .enqueue_package(
            spec(directory.path()),
            vec![file("a.bin", DownloadKind::Ftp)],
        )
        .await
        .expect("enqueue");
    scheduler
        .switch_bandwidth_profile(None, ManualEnd::Never, None)
        .await
        .expect("no limits");
    scheduler.schedule_runnable().await.expect("pass");
    assert!(
        looked_at(&database, files[0].id).await,
        "download now anyway"
    );
}

/// A running transfer that can resume is paused by a closed window and recorded; one that cannot
/// runs on. Once the window opens, the recorded file goes on, and a file somebody paused by hand
/// stays paused.
#[tokio::test]
async fn a_window_pauses_a_resumable_transfer_and_its_end_resumes_only_that() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let (package, files) = scheduler
        .enqueue_package(
            spec(directory.path()),
            vec![
                file("running.bin", DownloadKind::Ftp),
                file("video.mp4", DownloadKind::Media),
                file("mine.bin", DownloadKind::Ftp),
            ],
        )
        .await
        .expect("enqueue");
    let (running, video, mine) = (files[0].id, files[1].id, files[2].id);
    // Two transfers under way, as a worker leaves them; the third paused by hand.
    let mut tokens = Vec::new();
    for id in [running, video] {
        database
            .transition_download(id, DownloadState::Resolving)
            .await
            .expect("resolving");
        database
            .transition_download(id, DownloadState::Downloading)
            .await
            .expect("downloading");
        let token = CancellationToken::new();
        scheduler
            .active
            .lock()
            .await
            .tokens
            .insert(id, token.clone());
        tokens.push(token);
    }
    scheduler.pause(mine).await.expect("pause by hand");

    let closed = window_now(&scheduler, false).await;
    database
        .set_package_download_window(package.id, Some(closed))
        .await
        .expect("window");
    scheduler
        .supervise_download_windows()
        .await
        .expect("supervise");
    assert!(
        tokens[0].is_cancelled(),
        "the resumable transfer is stopped"
    );
    assert!(
        !tokens[1].is_cancelled(),
        "a media transfer runs to its end"
    );
    assert_eq!(
        scheduler.active.lock().await.reasons.get(&running),
        Some(&StopReason::Paused)
    );
    assert_eq!(
        scheduler.load_schedule_hold().await.expect("record"),
        ScheduleHold {
            files: vec![running]
        }
    );

    // The worker unwinds into `Paused`, as `transition_stopped` writes it.
    database
        .transition_download(running, DownloadState::Paused)
        .await
        .expect("paused");
    {
        let mut active = scheduler.active.lock().await;
        active.tokens.remove(&running);
        active.reasons.remove(&running);
    }
    // Still closed: nothing moves, the record stays.
    scheduler
        .supervise_download_windows()
        .await
        .expect("supervise");
    assert_eq!(
        scheduler.load_schedule_hold().await.expect("record").files,
        vec![running]
    );

    let open = window_now(&scheduler, true).await;
    database
        .set_package_download_window(package.id, Some(open))
        .await
        .expect("window");
    scheduler
        .supervise_download_windows()
        .await
        .expect("supervise");
    assert_eq!(
        state_of(&database, running).await,
        DownloadState::Queued,
        "resumed"
    );
    assert_eq!(
        state_of(&database, mine).await,
        DownloadState::Paused,
        "a pause somebody set is not lifted by the window"
    );
    assert!(
        scheduler
            .load_schedule_hold()
            .await
            .expect("record")
            .files
            .is_empty()
    );
}

/// A file the window paused that somebody resumed meanwhile is no longer the window's: the
/// record forgets it, and the dispatch pass keeps it waiting until the window opens.
#[tokio::test]
async fn a_file_resumed_by_hand_leaves_the_record() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = scheduler_over(directory.path()).await;
    let (package, files) = scheduler
        .enqueue_package(
            spec(directory.path()),
            vec![file("a.bin", DownloadKind::Ftp)],
        )
        .await
        .expect("enqueue");
    let closed = window_now(&scheduler, false).await;
    database
        .set_package_download_window(package.id, Some(closed))
        .await
        .expect("window");
    scheduler.pause(files[0].id).await.expect("pause");
    scheduler
        .store_schedule_hold(&ScheduleHold {
            files: vec![files[0].id],
        })
        .await
        .expect("record");
    scheduler.resume(files[0].id).await.expect("resume by hand");
    scheduler
        .supervise_download_windows()
        .await
        .expect("supervise");
    assert!(
        scheduler
            .load_schedule_hold()
            .await
            .expect("record")
            .files
            .is_empty()
    );
}
