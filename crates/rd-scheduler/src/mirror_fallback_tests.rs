//! RD-110-20 — a dead mirror does not stop the download.
//!
//! The cases run against a real database and a real scheduler, because every part of the
//! decision is persisted: which member holds the group's turn, which ones have been tried,
//! and what the group's verdict is once none is left. A restart reads exactly these rows, so
//! a test that mocked them would prove nothing about the restart.

use std::path::Path;

use rd_core::{DownloadFile, DownloadState, Failure, FailureKind};

use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

const HOSTS: [&str; 3] = [
    "https://first.example/release.mkv",
    "https://second.example/release.mkv",
    "https://third.example/release.mkv",
];

/// A scheduler over the temporary database in `directory`, started as a restart would.
///
/// Starting it is what runs the recovery sweeps, which is why the crash case below builds a
/// second one over the same file rather than calling the sweep by hand.
async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("mirrors.sqlite3"))
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
    (scheduler, database)
}

/// A paused package whose `members` links are mirrors of one another.
///
/// The first is the one the LinkGrabber selected, so it holds the group's turn; the rest wait
/// as `Skipped`, which is what the enqueue does for a group.
async fn mirror_group(
    directory: &Path,
    members: usize,
) -> (SchedulerHandle, rd_db::Database, Vec<DownloadFile>) {
    let (scheduler, database) = scheduler_over(directory).await;
    let spec = PackageSpec {
        name: "release".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = (0..members)
        .map(|index| FileSpec {
            source: HOSTS[index].parse().expect("url"),
            file_name: "release.mkv".to_owned(),
            size: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::default(),
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: Some("release".to_owned()),
            skipped: index > 0,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .collect::<Vec<_>>();
    let (_, created) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    (scheduler, database, created)
}

async fn state_of(database: &rd_db::Database, file: &DownloadFile) -> DownloadFile {
    database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("download still there")
}

fn offline() -> Failure {
    Failure::new(FailureKind::Offline, "no route to host")
}

fn not_a_file() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "download.not_a_file",
        "the server returned a web page",
    )
}

fn disk_full() -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        rd_http::LOCAL_IO_CODE,
        "No space left on device (os error 28)",
    )
}

#[tokio::test]
async fn a_mirror_that_went_offline_hands_the_turn_over_instead_of_failing_the_file() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;

    crate::failures::record_error(&scheduler, &files[0], offline())
        .await
        .expect("record");

    let given_up = state_of(&database, &files[0]).await;
    let successor = state_of(&database, &files[1]).await;
    assert_eq!(given_up.state, DownloadState::Failed);
    assert_eq!(
        successor.state,
        DownloadState::Queued,
        "the second mirror has to take the turn without anybody pasting the link again"
    );
    assert_eq!(
        given_up
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some(crate::mirrors::HANDOVER_CODE),
        "the switch is recorded on the mirror that was given up"
    );
}

#[tokio::test]
async fn a_page_instead_of_the_file_hands_the_mirror_turn_over() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;

    crate::failures::record_error(&scheduler, &files[0], not_a_file())
        .await
        .expect("record");

    assert_eq!(
        state_of(&database, &files[1]).await.state,
        DownloadState::Queued
    );
    let recorded = state_of(&database, &files[0])
        .await
        .last_error
        .expect("a reason");
    assert_eq!(
        recorded.code.as_deref(),
        Some(crate::mirrors::HANDOVER_CODE)
    );
    assert_eq!(
        recorded.params.get("reason_code").map(String::as_str),
        Some("download.not_a_file"),
        "the original reason stays readable under the handover"
    );
    assert_eq!(
        recorded.params.get("mirror").map(String::as_str),
        Some("second.example"),
        "and so does who took over"
    );
}

#[tokio::test]
async fn a_full_disk_does_not_burn_a_mirror() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;

    crate::failures::record_error(&scheduler, &files[0], disk_full())
        .await
        .expect("record");

    assert_eq!(
        state_of(&database, &files[1]).await.state,
        DownloadState::Skipped,
        "the next mirror writes to the same disk, so switching proves nothing"
    );
    assert_eq!(
        state_of(&database, &files[0])
            .await
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some(rd_http::LOCAL_IO_CODE),
        "the local cause is kept as it was, not dressed up as a handover"
    );
}

#[tokio::test]
async fn the_given_up_mirror_loses_its_partial_data() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;
    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == files[0].package_id)
        .expect("the package");
    let staging = Path::new(&package.destination).join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    let part = staging.join(format!("{}.part", files[0].id));
    tokio::fs::write(&part, b"half a file from the first hoster")
        .await
        .expect("write");

    crate::failures::record_error(&scheduler, &files[0], offline())
        .await
        .expect("record");

    assert!(
        !part.exists(),
        "bytes from the abandoned hoster must not be left where another source could be \
         laid on top of them"
    );
}

#[tokio::test]
async fn an_exhausted_mirror_group_ends_with_the_chosen_mirrors_reason() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 3).await;

    // The chosen mirror serves a page; the two that take over are simply gone.
    crate::failures::record_error(&scheduler, &files[0], not_a_file())
        .await
        .expect("first");
    let second = state_of(&database, &files[1]).await;
    crate::failures::record_error(&scheduler, &second, offline())
        .await
        .expect("second");
    let third = state_of(&database, &files[2]).await;
    assert_eq!(
        third.state,
        DownloadState::Queued,
        "the third mirror gets its turn before the group is called exhausted"
    );
    // A reason of its own, and one that is final, so the verdict below cannot be this one by
    // accident and the group really is out of attempts as well as out of mirrors.
    crate::failures::record_error(
        &scheduler,
        &third,
        Failure::coded(
            FailureKind::Permanent,
            "download.remote_changed",
            "the file on the server changed",
        ),
    )
    .await
    .expect("third");

    let verdict = state_of(&database, &files[2])
        .await
        .last_error
        .expect("a verdict");
    assert_eq!(
        verdict.code.as_deref(),
        Some(crate::mirrors::EXHAUSTED_CODE)
    );
    assert_eq!(verdict.params.get("mirrors").map(String::as_str), Some("3"));
    assert_eq!(
        verdict.params.get("reason_code").map(String::as_str),
        Some("download.not_a_file"),
        "the reason that counts is the chosen mirror's, not the last one tried"
    );
}

#[tokio::test]
async fn a_mirror_group_is_tried_once_per_round_and_does_not_loop() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;

    crate::failures::record_error(&scheduler, &files[0], offline())
        .await
        .expect("first");
    let second = state_of(&database, &files[1]).await;
    crate::failures::record_error(&scheduler, &second, offline())
        .await
        .expect("second");

    // Neither member is left waiting to be promoted: a group that is through is through,
    // and the queue says so instead of handing the turn round in circles.
    for file in &files {
        assert_ne!(
            state_of(&database, file).await.state,
            DownloadState::Skipped
        );
    }
}

#[tokio::test]
async fn a_restart_does_not_forget_which_mirrors_have_been_tried() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 3).await;
    crate::failures::record_error(&scheduler, &files[0], offline())
        .await
        .expect("record");
    drop(scheduler);
    drop(database);

    // Everything the decision rests on is a column, so a second process reads it back.
    let reopened = rd_db::Database::open(directory.path().join("mirrors.sqlite3"))
        .await
        .expect("reopen");
    let downloads = reopened.list_downloads().await.expect("list");
    let burnt = downloads
        .iter()
        .find(|file| file.id == files[0].id)
        .expect("the given-up mirror");
    assert!(
        burnt.last_error.is_some(),
        "the attempt that was made has to survive the restart"
    );
    let contenders: Vec<&DownloadFile> = downloads.iter().collect();
    let next = crate::mirrors::best_candidate(&contenders).expect("a candidate");
    assert_ne!(
        next.id, burnt.id,
        "a mirror that has already been through comes last, or the fallback loops"
    );
    assert_eq!(
        crate::mirrors::leader(burnt, &downloads).id,
        files[0].id,
        "the member the group started with is still the one whose reason counts"
    );
}

#[tokio::test]
async fn the_last_mirror_keeps_the_retries_a_single_link_would_have() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;
    crate::failures::record_error(&scheduler, &files[0], offline())
        .await
        .expect("first");
    let second = state_of(&database, &files[1]).await;

    crate::failures::record_error(&scheduler, &second, offline())
        .await
        .expect("second");

    let last = state_of(&database, &files[1]).await;
    assert_eq!(
        last.state,
        DownloadState::RetryWait,
        "a group of mirrors must not be less persistent than a single link"
    );
    assert!(last.next_retry_at.is_some());
}

/// Axis A of the recovery matrix for `scheduler.before_mirror_promoted`.
///
/// The handover is two writes through the serialized writer and cannot be one transaction, so
/// a process that stops between them leaves the group with nobody running and every remaining
/// mirror standing by. The dispatcher never looks at a skipped row, so without the start-up
/// sweep the group waits for a link that has already given up.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_before_the_mirror_is_promoted_is_repaired_by_the_next_start() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, files) = mirror_group(directory.path(), 2).await;

    let guard = rd_core::failpoint::FailpointGuard::once("scheduler.before_mirror_promoted");
    assert!(
        crate::failures::record_error(&scheduler, &files[0], offline())
            .await
            .is_err(),
        "the crash point did not stop the handover"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The state a restart finds, asserted rather than assumed.
    assert_eq!(
        state_of(&database, &files[0]).await.state,
        DownloadState::Failed
    );
    assert_eq!(
        state_of(&database, &files[1]).await.state,
        DownloadState::Skipped,
        "the crash point fired somewhere other than between the two writes"
    );
    drop(scheduler);
    drop(database);

    let (restarted, reopened) = scheduler_over(directory.path()).await;
    let successor = state_of(&reopened, &files[1]).await;
    assert!(
        crate::mirrors::holds_the_group_open(successor.state),
        "the group was left waiting for a link that had already given up, in state {:?}",
        successor.state
    );
    drop(restarted);
}
