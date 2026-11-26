//! An unrecognised indexer link in a package hand-over (RD-1480-05): its key stays on this
//! machine wherever the address carries it. The suite beside it, `collector_remote_job`, holds
//! the hand-over itself; this one shares its package and answer helpers.

use crate::collector_remote_job::{links_of, package, submitted_sources, uri, urls_in};
use crate::common::{post_json, test_harness};
use crate::nzb_remote_job::{account, install};
use crate::subscription_remote_job::indexer;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::http::StatusCode;
use serde_json::json;

/// With the indexer's key where no list of secret parameters looks -- in the path, in `r=`, or
/// at a configured indexer -- it goes as its document or stays, never as an address.
#[tokio::test]
async fn an_unchecked_indexer_link_never_goes_as_an_address_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    let torbox = account(&harness, "torbox").await;
    let asked = Arc::new(AtomicUsize::new(0));
    let server = indexer(asked.clone()).await;
    let address = server.addr();

    // One package each: the same document twice is one job.
    for (name, url) in [
        (
            "Query.S01",
            format!("http://{address}/getnzb?id=1&i=12&r=SECRET0123456789abcdef"),
        ),
        (
            "Api.S02",
            format!("http://{address}/getnzb?t=get&id=1&apikey=SECRET"),
        ),
    ] {
        let release = package(&harness, name, &[url]).await;
        let (status, answer) = post_json(
            &harness.router,
            &uri(&release),
            json!({ "account_id": torbox }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}: {answer}");
        assert_eq!(answer["stayed"], json!([]), "{name}: {answer}");
        assert_eq!(
            answer["handed_over"][0]["job"]["source_kind"], "container",
            "{name}: {answer}"
        );
        assert!(!answer.to_string().contains("SECRET"), "{name}: {answer}");
    }
    assert_eq!(asked.load(Ordering::SeqCst), 2);

    // A path that answers no NZB: the links stay, the one at the configured indexer too, and a
    // link the check placed elsewhere goes as it is.
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/indexers",
        json!({ "name": "Mine", "url": format!("http://{address}/api"), "api_key": "SECRET" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let keyed = [
        format!("http://{address}/getnzb/1.nzb&i=12&r=SECRET0123456789abcdef"),
        format!("http://{address}/get/1/SECRET"),
    ];
    let mut urls = keyed.to_vec();
    urls.push("https://files.example/Plain.S03E01.mkv".to_owned());
    let mixed = package(&harness, "Mixed.S03", &urls).await;
    let (status, answer) = post_json(
        &harness.router,
        &uri(&mixed),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let links = links_of(&harness, &mixed).await;
    let mut stayed = urls_in(&answer, "stayed", &links);
    stayed.sort();
    let mut expected = keyed.to_vec();
    expected.sort();
    assert_eq!(stayed, expected, "{answer}");
    assert!(
        answer["stayed"]
            .as_array()
            .expect("stayed")
            .iter()
            .all(|entry| entry["code"]
                .as_str()
                .is_some_and(|code| code.starts_with("collector.nzb_"))),
        "{answer}"
    );
    let sources = submitted_sources(&harness).await;
    assert!(sources.contains("Plain.S03E01.mkv"), "{sources}");
    assert!(sources.contains("msg-1@example"), "{sources}");
    assert!(!sources.contains("SECRET"), "{sources}");
}
