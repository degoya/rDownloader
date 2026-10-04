//! Audit 1.9.1, INTAKE-05: an rclone that hangs is killed after the stall limit, a shutdown
//! stops it, and a log line that is not UTF-8 does not end the reading of its output.

use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    ExtractionConfig, ExtractionService,
    rclone_job::{Ended, UploadContext, UploadMode, execute},
};

/// A stand-in for rclone that runs `body` whatever it is asked.
fn fake_rclone(directory: &Path, body: &str) -> String {
    let path = directory.join("rclone");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("fake rclone");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path.to_string_lossy().into_owned()
}

async fn service(temp: &Path) -> ExtractionService {
    let database = Database::open(temp.join("extract.sqlite"))
        .await
        .expect("database");
    ExtractionService::start(
        database,
        ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    )
}

async fn run_fake(body: &str, stop: &CancellationToken, stall: Duration) -> Ended {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = fake_rclone(temp.path(), body);
    let service = service(temp.path()).await;
    let context = UploadContext {
        remote: "gdrive:downloads",
        mode: UploadMode::Copy,
        package_name: "Release",
        directory: temp.path(),
        executable: Some(&tool),
        vendor_directory: None,
        bwlimit: None,
    };
    let ended = tokio::time::timeout(
        Duration::from_secs(20),
        execute(
            &service.inner,
            "0199aa00-0000-7000-8000-000000000001",
            &context,
            "gdrive:downloads/Release",
            stop,
            stall,
        ),
    )
    .await
    .expect("rclone is not waited for forever")
    .expect("rclone ran");
    service.shutdown().await;
    ended
}

#[tokio::test]
async fn an_rclone_that_moves_nothing_is_killed_at_the_stall_limit() {
    let stall = Duration::from_millis(300);
    let ended = run_fake("exec sleep 30", &CancellationToken::new(), stall).await;
    assert_eq!(ended, Ended::Stalled(stall));
}

#[tokio::test]
async fn a_shutdown_stops_a_running_rclone() {
    let stop = CancellationToken::new();
    let trigger = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });
    let ended = run_fake("exec sleep 30", &stop, Duration::from_secs(60)).await;
    assert_eq!(ended, Ended::Stopped);
}

#[tokio::test]
async fn a_log_line_that_is_not_utf8_does_not_end_the_reading() {
    let ended = run_fake(
        r"printf 'legacy \377 name\nlast words\n' >&2; exit 3",
        &CancellationToken::new(),
        Duration::from_secs(60),
    )
    .await;
    let Ended::Failed(text) = ended else {
        panic!("a failing rclone fails, got {ended:?}");
    };
    assert!(text.starts_with("rclone exit status 3"), "{text}");
    assert!(
        text.contains("last words"),
        "the line after the bad one is read: {text}"
    );
}
