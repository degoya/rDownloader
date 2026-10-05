//! A torrent move stopped on either side of its commit (RD-1100-10, recovery matrix).
//!
//! Both cases stop a seed's move at its crash point, start a second service over the same data
//! and let its `recover` settle the move. Within one filesystem every file is renamed, so each
//! case also puts back the state a move across two filesystems leaves — the original kept beside
//! its verified copy, a temporary copy beside a target — to show that it settles the same way.

use rd_core::failpoint::FailpointGuard;

use super::tests::Seed;
use crate::registry::TorrentPhase;

/// `torrent.before_relocation_commit`: the files are in the new folder, the package still names
/// the old one. The next start takes the move back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_stopped_before_its_commit_is_taken_back_by_the_next_start() {
    let mut seed = Seed::new().await;
    {
        let guard = FailpointGuard::once("torrent.before_relocation_commit");
        assert!(
            seed.service
                .relocate(seed.id, seed.to.clone())
                .await
                .is_err()
        );
        assert!(guard.fired(), "the crash point was never reached");
    }
    assert!(Seed::holds_both(&seed.to), "the files were not placed");
    assert_eq!(seed.destination().await, seed.from);
    assert!(
        seed.service.job_state(seed.id).await.relocation.is_some(),
        "the journal is what the next start goes by"
    );
    // A copy across filesystems keeps its original until the release, and a stop during a copy
    // leaves its temporary name.
    std::fs::create_dir_all(seed.from.join("sub")).expect("old folder");
    std::fs::copy(
        seed.to.join("sub").join("b.bin"),
        seed.from.join("sub").join("b.bin"),
    )
    .expect("original beside its copy");
    std::fs::write(rd_files::move_temporary_of(&seed.to.join("a.bin")), b"a")
        .expect("temporary copy");

    seed.restart().await;

    assert!(Seed::holds_both(&seed.from), "the files are not back");
    assert!(!seed.to.exists(), "something was left in the new folder");
    assert_eq!(seed.destination().await, seed.from);
    let state = seed.service.job_state(seed.id).await;
    assert!(state.relocation.is_none(), "the journal was not cleared");
    assert!(state.relocation_error.is_some(), "the reason was not kept");
    assert_eq!(seed.phase().await, Some(TorrentPhase::Seeding));
}

/// `torrent.after_relocation_commit`: the package names the new folder, the originals have not
/// been released. The next start finishes the move.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_stopped_after_its_commit_is_finished_by_the_next_start() {
    let mut seed = Seed::new().await;
    {
        let guard = FailpointGuard::once("torrent.after_relocation_commit");
        assert!(
            seed.service
                .relocate(seed.id, seed.to.clone())
                .await
                .is_err()
        );
        assert!(guard.fired(), "the crash point was never reached");
    }
    assert_eq!(seed.destination().await, seed.to);
    // The original of a copy across filesystems is still there until the release.
    std::fs::create_dir_all(&seed.from).expect("old folder");
    std::fs::copy(seed.to.join("a.bin"), seed.from.join("a.bin")).expect("original");

    seed.restart().await;

    assert!(
        Seed::holds_both(&seed.to),
        "the files are not in the new folder"
    );
    assert!(!seed.from.exists(), "the old folder is not empty");
    assert_eq!(seed.destination().await, seed.to);
    let state = seed.service.job_state(seed.id).await;
    assert!(state.relocation.is_none(), "the journal was not cleared");
    assert!(state.relocation_error.is_none());
    assert_eq!(seed.phase().await, Some(TorrentPhase::Seeding));
}
