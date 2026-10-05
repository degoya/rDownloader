use std::{collections::HashMap, path::PathBuf, time::Duration};

use rd_core::HotFolderId;

use super::{
    nzb_duplicate_record, nzb_failure_record, poll_interval_of, still_running,
    validate_hotfolder_settings,
};
use crate::dto::SettingsResponse;

/// A watcher that ended no longer blocks a restart; a running one still does.
#[tokio::test]
async fn a_finished_watcher_is_taken_out_so_the_folder_can_start_again() {
    let mut tasks = HashMap::new();
    let dead = HotFolderId::new();
    let alive = HotFolderId::new();
    let finished: tokio::task::JoinHandle<anyhow::Result<()>> =
        tokio::spawn(async { anyhow::bail!("destination escapes") });
    while !finished.is_finished() {
        tokio::task::yield_now().await;
    }
    tasks.insert(dead, finished);
    let token = tokio_util::sync::CancellationToken::new();
    let waiting = token.clone();
    tasks.insert(
        alive,
        tokio::spawn(async move {
            waiting.cancelled().await;
            Ok(())
        }),
    );

    assert!(!still_running(&mut tasks, dead).await);
    assert!(!tasks.contains_key(&dead));
    assert!(still_running(&mut tasks, alive).await);
    assert!(!still_running(&mut tasks, HotFolderId::new()).await);
    token.cancel();
}

/// RD-110-31: the bounds, and the code a value outside them is refused with.
#[test]
fn the_poll_interval_is_bounded() {
    let mut settings = SettingsResponse::default();
    assert!(validate_hotfolder_settings(&settings).is_ok());
    assert_eq!(poll_interval_of(&settings), Duration::from_secs(30));
    for seconds in [5, 3600] {
        settings.hotfolder_poll_seconds = seconds;
        assert!(validate_hotfolder_settings(&settings).is_ok(), "{seconds}");
    }
    for seconds in [0, 4, 3601] {
        settings.hotfolder_poll_seconds = seconds;
        assert_eq!(
            validate_hotfolder_settings(&settings)
                .expect_err("outside the range")
                .code(),
            "settings.hotfolder_poll_invalid",
            "{seconds}"
        );
    }
}

fn failure(name: &str) -> rd_hotfolder::FailedIntake {
    rd_hotfolder::FailedIntake {
        source_path: PathBuf::from("/watch").join(name),
        sha256: "a".repeat(64),
        failed_path: PathBuf::from("/watch/failed").join(name),
        reason: "NZB could not be parsed".to_owned(),
    }
}

#[test]
fn a_refused_nzb_becomes_a_record_under_the_name_a_successful_one_would_have() {
    let record = nzb_failure_record(&failure("Release{{secret}}.nzb")).expect("record");
    assert_eq!(record.name, "Release.nzb");
    assert_eq!(record.sha256, "a".repeat(64));
    // The watched file's own path, in the platform's spelling (`\` before the name on Windows).
    let source = PathBuf::from("/watch").join("Release{{secret}}.nzb");
    assert_eq!(
        record.source_path.as_deref(),
        Some(source.to_string_lossy().as_ref())
    );
    assert_eq!(record.error, "NZB could not be parsed");
}

/// `nzb_imports` is the NZB table; the other three container kinds have no row to leave.
#[test]
fn a_refused_container_leaves_no_nzb_record() {
    for name in ["batch.torrent", "links.dlc", "links.ccf", "links.rsdf"] {
        assert!(
            nzb_failure_record(&failure(name)).is_none(),
            "{name} produced an NZB record"
        );
    }
    assert!(nzb_failure_record(&failure("release.NZB")).is_some());
}

fn duplicate(name: &str) -> rd_hotfolder::DuplicateIntake {
    rd_hotfolder::DuplicateIntake {
        source_path: PathBuf::from("/watch").join(name),
        sha256: "a".repeat(64),
        processed_path: PathBuf::from("/watch/processed").join(name),
    }
}

/// RA-IN-02: a duplicate whose first import is gone is recorded like a refused NZB; while
/// the import is there, it is the record, and another kind of file has no NZB row.
#[test]
fn a_duplicate_drop_is_recorded_only_when_its_import_is_gone() {
    let record = nzb_duplicate_record(&duplicate("Release.nzb"), false).expect("record");
    assert_eq!(record.name, "Release.nzb");
    assert_eq!(record.sha256, "a".repeat(64));
    assert!(record.error.contains("processed"), "{}", record.error);

    assert!(nzb_duplicate_record(&duplicate("Release.nzb"), true).is_none());
    assert!(nzb_duplicate_record(&duplicate("batch.torrent"), false).is_none());
}
