//! A server that cannot answer right now has not said the article is gone (RD-108-29).
//!
//! The field: one provider answered `400 Archive server temporarily offline.` to a quarter
//! of all `BODY` commands. Every one of them was read as "this server does not have the
//! article", the segment was written as a hole of zeros and the file was reported complete -
//! 2313 segments over 180 files in a single afternoon, far past what PAR2 can repair, and the
//! archives came out broken. SABnzbd survives the same server because it reconnects and asks
//! again. These tests are that difference.

use rd_core::{Failure, FailureKind, NzbSegmentState};

use crate::test_support::{Faults, FixtureBehaviour, article_set, completed, reload, run, states};

/// How often the fixture was asked for each article.
fn attempts_per_article(requests: &[String]) -> std::collections::BTreeMap<&str, usize> {
    let mut counted = std::collections::BTreeMap::new();
    for request in requests {
        *counted.entry(request.as_str()).or_insert(0) += 1;
    }
    counted
}

#[tokio::test]
async fn a_refusal_the_server_takes_back_costs_no_article() {
    let set = article_set(6, &[]);
    let run = run(&set, 2, FixtureBehaviour::with(Faults::transient(1))).await;

    let (path, missing) = completed(run.outcome);
    assert_eq!(missing, 0, "a 400 is not a missing article");
    assert_eq!(
        tokio::fs::read(&path).await.expect("assembled file"),
        set.expected
    );
    for (article, attempts) in attempts_per_article(&run.log.requests()) {
        assert_eq!(attempts, 2, "{article} should have been asked twice");
    }
}

#[tokio::test]
async fn a_corrupt_body_is_asked_for_again_on_the_same_server() {
    let set = article_set(4, &[]);
    let run = run(&set, 2, FixtureBehaviour::with(Faults::corrupt(1))).await;

    let (path, missing) = completed(run.outcome);
    assert_eq!(missing, 0, "a broken checksum is not a missing article");
    assert_eq!(
        tokio::fs::read(&path).await.expect("assembled file"),
        set.expected
    );
}

#[tokio::test]
async fn a_server_that_stays_down_fails_the_file_instead_of_filling_it_with_zeros() {
    let set = article_set(4, &[]);
    let run = run(&set, 2, FixtureBehaviour::with(Faults::transient(99))).await;

    let error = run
        .outcome
        .err()
        .expect("a server that never answers fails");
    let failure = error
        .downcast_ref::<Failure>()
        .expect("a coded failure the queue can retry");
    assert_eq!(failure.code.as_deref(), Some("usenet.server_unavailable"));
    assert!(
        matches!(failure.category, FailureKind::Transient { .. }),
        "{:?} must be retryable",
        failure.category
    );
    assert!(
        failure.category.is_retryable(),
        "the queue has to come back to this file"
    );
    assert!(
        states(&run.database, &run.file)
            .await
            .iter()
            .all(|state| *state != NzbSegmentState::Completed),
        "nothing was assembled, so nothing is complete"
    );
    assert!(
        reload(&run.database, &run.file).await.output_path.is_none(),
        "no file was placed in the destination"
    );
}

/// The counter-check: `430` on every server is still a hole, and PAR2 is still what repairs
/// it. Only the answer that says "not here" may cost bytes.
#[tokio::test]
async fn an_article_no_server_has_is_still_a_hole() {
    let set = article_set(5, &[3]);
    let run = run(&set, 2, FixtureBehaviour::default()).await;

    let (path, missing) = completed(run.outcome);
    assert_eq!(missing, 1);
    assert_eq!(
        tokio::fs::read(&path).await.expect("assembled file"),
        set.expected
    );
}
