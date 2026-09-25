//! The cancellation boundary around enqueue-and-enrich, and the rollback underneath it.
//!
//! Turning a LinkGrabber package into a queue package is a sequence of separate writes: the
//! links are locked, one queue row is written per file, what the enrichers found is carried
//! onto those rows, the reviewed torrent selection follows, and only then is the lock
//! released. Axum drops a handler's future the instant the client disconnects, so without a
//! boundary around the whole sequence a disconnect can stop it between any two of those
//! writes — and every stopping point leaves something a user has to repair by hand: a package
//! that reads as complete and is short, links locked forever, a package with no enrichment.
//!
//! Two properties have to hold together, and the interesting thing about them is that the
//! obvious fix for one breaks the other. Detaching only the scheduler's half was tried in an
//! earlier pass and reverted, because the enrichment that follows it belongs to the same
//! operation and a task boundary in the middle let a reader see the package without it. So:
//!
//! 1. **All or nothing.** However the request is abandoned, what is left is either no package
//!    at all or the whole one, never a package holding some of its files.
//! 2. **Never visible without its enrichment.** A package that exists carries the fields its
//!    candidates carried.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{Harness, test_harness};
use tower::ServiceExt;

/// Two links, both already online, both carrying an enricher field.
async fn submit(harness: &Harness) -> rd_core::CollectorPackageId {
    let urls = [
        "https://one.example/release.part1.rar",
        "https://one.example/release.part2.rar",
    ];
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: urls.iter().map(|_| None).collect(),
            file_names: urls
                .iter()
                .map(|url| Some(url.rsplit('/').next().expect("name").to_owned()))
                .collect(),
            sizes: urls.iter().map(|_| None).collect(),
            requests: urls.iter().map(|_| None).collect(),
            body_refs: urls.iter().map(|_| None).collect(),
            urls: urls.iter().map(|url| url.parse().expect("url")).collect(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    for candidate in harness
        .database
        .list_candidates()
        .await
        .expect("candidates")
    {
        harness
            .database
            .set_candidate_enrichment(
                candidate.id,
                vec![rd_core::EnrichmentField {
                    name: "imdb.score".to_owned(),
                    value: "7.4".to_owned(),
                    plugin_id: "imdb-enricher".to_owned(),
                    fetched_at: chrono::Utc::now(),
                }],
            )
            .await
            .expect("enrichment");
    }
    harness
        .database
        .list_collector_packages()
        .await
        .expect("packages")
        .first()
        .expect("one collector package")
        .id
}

fn enqueue_request(id: rd_core::CollectorPackageId) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/v1/collector/packages/{id}/enqueue"))
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .expect("request")
}

/// Drives one request for `turns` scheduler turns and then drops it, as a disconnect does.
///
/// Counted turns rather than a wall-clock timeout: a timeout abandons the request at a
/// different point on every machine and at every load, which is how a cancellation test ends
/// up passing for reasons unrelated to what it claims. Yielding between polls also gives the
/// serialized database writer the turns it needs to make each step real, so the abandoning
/// happens *between* writes rather than before all of them.
async fn abandon_after(router: &Router, request: Request<Body>, turns: usize) -> bool {
    let mut call = Box::pin(router.clone().oneshot(request));
    for _ in 0..turns {
        tokio::select! {
            biased;
            _ = &mut call => return true,
            () = tokio::task::yield_now() => {}
        }
    }
    false
}

/// Waits until nothing is holding the enqueue claim any more.
///
/// The claim is taken first and released last, so its absence is the whole operation being
/// over — including the detached part that outlives the abandoned request. The fixed wait in
/// front of it is there because "not claimed yet" looks exactly like "claimed and released".
async fn settle(harness: &Harness) {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    for _ in 0..500 {
        let claimed = harness
            .database
            .list_candidates()
            .await
            .expect("candidates")
            .into_iter()
            .any(|candidate| candidate.state == rd_core::LinkCandidateState::Resolving);
        if !claimed {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the enqueue never released its claim on the links");
}

/// The two properties, asserted against whatever the database ended up holding.
async fn assert_all_or_nothing(harness: &Harness, context: &str) {
    let packages = harness.database.list_packages().await.expect("packages");
    let downloads = harness.database.list_downloads().await.expect("downloads");
    match packages.len() {
        0 => assert!(
            downloads.is_empty(),
            "{context}: {} queue rows survived without a package",
            downloads.len()
        ),
        1 => {
            assert_eq!(
                downloads.len(),
                2,
                "{context}: the package holds part of its file set, which reads in the \
                 interface exactly like a package that is simply short"
            );
            assert_eq!(
                packages[0].enrichment.len(),
                1,
                "{context}: the package became visible without the fields its links carried"
            );
            for download in &downloads {
                assert_eq!(
                    download.enrichment.len(),
                    1,
                    "{context}: a queue row became visible without its enrichment"
                );
            }
        }
        other => panic!("{context}: {other} packages came out of one enqueue"),
    }
}

/// The uninterrupted run: when the response is written, everything is already there.
///
/// This is the half a task boundary inside the operation used to break — the response said
/// `201` and named a package whose enrichment had not landed yet.
#[tokio::test]
async fn a_completed_enqueue_answers_with_the_enrichment_already_written() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let id = submit(&harness).await;

    let response = harness
        .router
        .clone()
        .oneshot(enqueue_request(id))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);

    assert_all_or_nothing(&harness, "uninterrupted").await;
}

/// Abandoning the request at any point leaves a whole package or none.
///
/// The turn counts are spread across the operation on purpose: the early ones abandon it
/// before anything is claimed, the later ones somewhere inside the run of `create_download`
/// calls and the enrichment that follows them. The assertion is the same for all of them,
/// because "either nothing happened or everything did" is the property, not a particular
/// outcome.
#[tokio::test]
async fn an_abandoned_request_never_leaves_half_a_package() {
    for turns in [1usize, 2, 3, 5, 8, 13, 21, 34] {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = test_harness(directory.path()).await;
        let id = submit(&harness).await;

        abandon_after(&harness.router, enqueue_request(id), turns).await;
        settle(&harness).await;

        assert_all_or_nothing(&harness, &format!("abandoned after {turns} turns")).await;
    }
}
