//! A failed verdict held until the settled set shows whether it carries PAR2.

use rd_core::{ImportMode, IngressSource};

use super::nzb_file;
use crate::{Database, NewNzbImport};

/// Two obfuscated rows of one package, ready for a verdict to be held open on the first.
///
/// Neither subject says anything about PAR2 — the case RD-108-23 named as its remaining
/// limit — so nothing but the content of the assembled files can answer the question.
async fn obfuscated_pair(
    directory: &std::path::Path,
    digest: &str,
    names: &[&str],
) -> (Database, rd_core::PackageId, Vec<rd_core::DownloadFile>) {
    let database = Database::open(directory.join("verdict.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "obfuscated.nzb".to_owned(),
            sha256: digest.repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: names.iter().map(|name| nzb_file(name)).collect(),
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .collect();
    (database, package.id, rows)
}

/// Drives a row to `Verifying` and holds its verdict open, the way the Usenet runner does
/// when a file is assembled with holes (RD-108-24).
async fn defer_in_verifying(database: &Database, id: rd_core::DownloadId, missing: usize) {
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
    ] {
        database
            .transition_download(id, state)
            .await
            .expect("on its way");
    }
    database
        .defer_par2_verdict(id, missing)
        .await
        .expect("defer the verdict");
    database
        .transition_download(id, rd_core::DownloadState::Verifying)
        .await
        .expect("held for the verdict");
}

fn row_of(rows: &[rd_core::DownloadFile], name: &str) -> rd_core::DownloadFile {
    rows.iter()
        .find(|file| file.file_name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no row for {name}"))
}

async fn state_of(database: &Database, id: rd_core::DownloadId) -> rd_core::DownloadFile {
    database
        .get_download(id)
        .await
        .expect("row")
        .expect("row exists")
}

/// RD-108-24: the verdict waits for the set and then sends the file to repair.
///
/// The payload finishes first with a hole and nothing in the package has declared itself as
/// PAR2 yet — the exact moment at which the old code said `usenet.segments_missing_no_par2`
/// about a set that carries PAR2. The row waits instead, and when the second file turns out
/// to be recovery data the verdict is `Completed`, which is what hands it to the repair.
#[tokio::test]
async fn a_held_verdict_completes_once_the_settled_set_turns_out_to_carry_par2() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d1", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");

    defer_in_verifying(&database, payload.id, 2).await;

    let held = state_of(&database, payload.id).await;
    assert_eq!(
        held.state,
        rd_core::DownloadState::Verifying,
        "a sibling is still queued, so nothing is decided yet"
    );
    assert_eq!(
        held.last_error
            .as_ref()
            .and_then(|error| error.code.clone()),
        Some("usenet.segments_missing_awaiting_par2".to_owned()),
        "the row says what it is waiting for"
    );

    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(other.id, state)
            .await
            .expect("sibling on its way");
    }
    database
        .settle_nzb_recovery(other.id, "d4e5f6.bin".to_owned(), true)
        .await
        .expect("settle the sibling as PAR2 by content");
    database
        .complete_download(other.id, "d4e5f6.bin".to_owned(), None)
        .await
        .expect("sibling complete");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(
        decided.state,
        rd_core::DownloadState::Completed,
        "the settled set carries PAR2, so the hole is the repair's business: {:?}",
        decided.last_error
    );
    assert!(
        decided.last_error.is_none(),
        "the note about the open verdict is cleared with the verdict"
    );
}

/// RD-108-24: the limit this job must not move — a set without PAR2 still fails, with the
/// same code and the same `missing` count, as soon as the set has settled.
#[tokio::test]
async fn a_held_verdict_fails_with_the_old_message_when_the_settled_set_has_no_par2() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d2", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");

    defer_in_verifying(&database, payload.id, 3).await;
    assert_eq!(
        state_of(&database, payload.id).await.state,
        rd_core::DownloadState::Verifying
    );

    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(other.id, state)
            .await
            .expect("sibling on its way");
    }
    database
        .complete_download(other.id, "d4e5f6.bin".to_owned(), None)
        .await
        .expect("sibling complete");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(decided.state, rd_core::DownloadState::Failed);
    let failure = decided.last_error.expect("a failure is recorded");
    assert_eq!(
        failure.code.as_deref(),
        Some("usenet.segments_missing_no_par2"),
        "the message a set without PAR2 has always given"
    );
    assert_eq!(
        failure.params.get("missing").map(String::as_str),
        Some("3"),
        "the count survives the wait"
    );
}

/// RD-108-24: a restart in the middle of the open verdict keeps the file and takes the
/// verdict, rather than fetching the whole file again to ask the same question.
///
/// The row is left exactly as a crash between the two writes leaves it: `Verifying`, marked,
/// and with nothing that would transition it again.
#[tokio::test]
async fn a_restart_decides_a_held_verdict_whose_set_has_nothing_left_running() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) = obfuscated_pair(directory.path(), "d3", &["a1b2c3.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(payload.id, state)
            .await
            .expect("on its way");
    }
    database
        .defer_par2_verdict(payload.id, 1)
        .await
        .expect("defer the verdict");

    database.recover_interrupted().await.expect("recovery");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(
        decided.state,
        rd_core::DownloadState::Failed,
        "nothing is left that could bring PAR2, so the verdict is final"
    );
    assert_eq!(
        decided
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some("usenet.segments_missing_no_par2")
    );
}

/// RD-108-24: a restart does not requeue a row whose verdict is open, and does not decide it
/// while the package still has work queued.
#[tokio::test]
async fn a_restart_leaves_a_held_verdict_open_while_a_sibling_is_still_queued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d4", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");
    defer_in_verifying(&database, payload.id, 2).await;

    database.recover_interrupted().await.expect("recovery");

    let held = state_of(&database, payload.id).await;
    assert_eq!(
        held.state,
        rd_core::DownloadState::Verifying,
        "the assembled file is kept; requeueing it would fetch it all again"
    );
    assert_eq!(
        held.last_error.and_then(|failure| failure.code).as_deref(),
        Some("usenet.segments_missing_awaiting_par2")
    );
    assert_eq!(
        state_of(&database, other.id).await.state,
        rd_core::DownloadState::Queued,
        "the sibling is what the verdict is still waiting for"
    );
}

/// RD-108-24: removing the sibling a held verdict waits for decides it.
///
/// The owner's case: a package whose other files were removed while one of them waited for
/// the rest of the set. Nothing else would ever move in that package again, so the removal
/// itself has to ask the question, or the row sits in `Verifying` with no worker for good.
#[tokio::test]
async fn removing_the_last_sibling_decides_a_held_verdict() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, package, rows) =
        obfuscated_pair(directory.path(), "d5", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");
    defer_in_verifying(&database, payload.id, 4).await;
    assert_eq!(
        state_of(&database, payload.id).await.state,
        rd_core::DownloadState::Verifying,
        "the queued sibling keeps the verdict open"
    );

    database
        .delete_download(other.id)
        .await
        .expect("remove the queued sibling");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(
        decided.state,
        rd_core::DownloadState::Failed,
        "nothing is left that could bring PAR2"
    );
    assert_eq!(
        decided
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some("usenet.segments_missing_no_par2")
    );
    assert!(
        database
            .list_packages()
            .await
            .expect("packages")
            .iter()
            .any(|remaining| remaining.id == package),
        "the package keeps its decided file"
    );
}

/// A row waiting for its set's verdict can be cancelled and then removed like any other.
#[tokio::test]
async fn a_held_verdict_can_be_cancelled_and_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d6", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    defer_in_verifying(&database, payload.id, 1).await;

    database
        .transition_download(payload.id, rd_core::DownloadState::Cancelled)
        .await
        .expect("a waiting row can be cancelled");
    database
        .delete_download(payload.id)
        .await
        .expect("a cancelled row can be removed");
    assert!(
        database
            .get_download(payload.id)
            .await
            .expect("lookup")
            .is_none()
    );
}
