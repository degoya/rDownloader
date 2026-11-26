//! RD-1460-03: a hit of an indexer subscription handed straight to a remote-job provider.
//!
//! The hit's NZB is fetched from the indexer here and submitted as a container through the path
//! an NZB import of the LinkGrabber takes (`rd_api_core::nzb_hand_over`); afterwards the hit is
//! decided as after "queue". The refusals that need no plugin are checked without components;
//! the ones that read what a plugin declares, and the hand-over itself, install the real
//! `torbox-jobs` (takes NZBs) and `realdebrid-torrents` (does not) and are named
//! `on_real_components`, so the `no-components` profile leaves them out.

use crate::common;
use crate::nzb_remote_job::{account, install};

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Router, extract::State, http::StatusCode, routing::get};
use common::{get_json, post_json, test_harness};
use serde_json::{Value, json};

/// An NZB without a password of its own: the hit's announced one is added when it goes.
const NZB: &str = r#"<?xml version="1.0"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="poster" subject="Example.Release.1080p &quot;example.r00&quot; yEnc (1/1)">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="500000" number="1">msg-1@example</segment></segments>
  </file>
</nzb>"#;

/// The indexer: the NZB, and the refusal an exhausted allowance answers with inside a `200`.
/// `asked` counts every download, so a refusal can be shown to come before the indexer is asked.
pub(crate) async fn indexer(asked: Arc<AtomicUsize>) -> rd_testkit::TestServer {
    let app = Router::new()
        .route(
            "/getnzb",
            get(|State(asked): State<Arc<AtomicUsize>>| async move {
                asked.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "application/x-nzb")], NZB)
            }),
        )
        .route(
            "/getnzb-refused",
            get(|State(asked): State<Arc<AtomicUsize>>| async move {
                asked.fetch_add(1, Ordering::SeqCst);
                (
                    [
                        ("content-type", "application/x-nzb"),
                        ("x-dnzb-rcode", "450"),
                        ("x-dnzb-rtext", "Request limit reached"),
                    ],
                    "<error code=\"450\" description=\"Request limit reached\"/>",
                )
            }),
        )
        .with_state(asked);
    rd_testkit::serve(app).await
}

/// An indexer subscription, switched off so no poll runs beside the test, with one hit per
/// `(key, path, state)` on the test indexer.
async fn subscription_with_hits(
    harness: &common::Harness,
    server: &rd_testkit::TestServer,
    hits: &[(&str, &str, rd_core::SubscriptionItemState)],
) -> (String, Vec<rd_core::SubscriptionItem>) {
    let address = server.addr();
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/subscriptions",
        json!({
            "name": "My Indexer",
            "url": format!("http://{address}/api?t=search&cat=5030"),
            "kind": "indexer",
            "enabled": false,
            "mode": "review",
            "interval_seconds": 3_600,
            "filters": {},
            "backlog": { "mode": "review_all" },
            "api_key": "super-secret-indexer-key"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let recorded = harness
        .database
        .record_subscription_items(
            id.parse().expect("subscription id"),
            hits.iter()
                .map(|(key, path, state)| rd_db::NewSubscriptionItem {
                    item_key: (*key).to_owned(),
                    title: "Example.Release.1080p".to_owned(),
                    url: format!("http://{address}{path}?apikey=SECRET")
                        .parse()
                        .expect("url"),
                    published_at: None,
                    duration_seconds: None,
                    state: *state,
                    reason: None,
                    source_category: None,
                    media_type: Some("application/x-nzb".to_owned()),
                    attributes: Default::default(),
                    password: Some("hit-secret".to_owned()),
                })
                .collect(),
        )
        .await
        .expect("record");
    (id, recorded)
}

fn hand_over_uri(item: &rd_core::SubscriptionItem) -> String {
    format!("/api/v1/subscriptions/items/{}/remote-job", item.id)
}

/// The state of every hit of the subscription, by id.
async fn states(harness: &common::Harness, subscription: &str) -> Value {
    let (status, page) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{subscription}/items/page?state=all&limit=200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap_or_default().to_owned(),
                item["state"].clone(),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The refusals that come before the indexer is asked: a hit that does not exist or is decided,
/// an account that does not exist or runs no remote jobs.
#[tokio::test]
async fn a_hit_that_cannot_go_is_refused_before_the_indexer_is_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let asked = Arc::new(AtomicUsize::new(0));
    let server = indexer(asked.clone()).await;
    let (subscription, hits) = subscription_with_hits(
        &harness,
        &server,
        &[
            (
                "waiting",
                "/getnzb",
                rd_core::SubscriptionItemState::Pending,
            ),
            (
                "decided",
                "/getnzb",
                rd_core::SubscriptionItemState::Dismissed,
            ),
        ],
    )
    .await;
    let waiting = hits
        .iter()
        .find(|hit| hit.item_key == "waiting")
        .expect("hit");
    let decided = hits
        .iter()
        .find(|hit| hit.item_key == "decided")
        .expect("hit");
    let torbox = account(&harness, "torbox").await;

    let missing = format!(
        "/api/v1/subscriptions/items/{}/remote-job",
        rd_core::SubscriptionItemId::new()
    );
    let (status, answer) =
        post_json(&harness.router, &missing, json!({ "account_id": torbox })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "subscription.item_not_found");

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(decided),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "subscription.remote_job_item_decided");

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(waiting),
        json!({ "account_id": rd_core::AccountId::new() }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "remote_job.no_account");

    // No remote-job plugin is installed, so no account runs remote jobs.
    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(waiting),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_not_remote_account");

    assert_eq!(asked.load(Ordering::SeqCst), 0, "the indexer was asked");
    let states = states(&harness, &subscription).await;
    assert_eq!(states[waiting.id.to_string()], "pending", "{states}");
}

/// The owner's request: the hit's NZB goes to TorBox as a container named after the hit, with
/// the password the indexer announced, and the hit is decided. A second hit of the same release
/// meets the duplicate guard and is decided too; a decided hit is refused.
#[tokio::test]
async fn an_indexer_hit_reaches_a_provider_that_takes_nzbs_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    install(&harness, "realdebrid-torrents").await;
    let torbox = account(&harness, "torbox").await;
    let realdebrid = account(&harness, "realdebrid").await;
    let asked = Arc::new(AtomicUsize::new(0));
    let server = indexer(asked.clone()).await;
    let (subscription, hits) = subscription_with_hits(
        &harness,
        &server,
        &[
            ("first", "/getnzb", rd_core::SubscriptionItemState::Pending),
            ("again", "/getnzb", rd_core::SubscriptionItemState::Pending),
        ],
    )
    .await;
    let first = hits
        .iter()
        .find(|hit| hit.item_key == "first")
        .expect("hit");
    let again = hits
        .iter()
        .find(|hit| hit.item_key == "again")
        .expect("hit");

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(first),
        json!({ "account_id": realdebrid }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_no_nzb");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the indexer was asked");

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(first),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["item_id"], first.id.to_string());
    assert_eq!(answer["state"], "queued");
    assert_eq!(answer["already_running"], false);
    assert_eq!(answer["job"]["source_kind"], "container");
    assert_eq!(answer["job"]["source_name"], "Example.Release.1080p.nzb");
    assert_eq!(answer["job"]["account_id"], torbox.to_string());
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    // The provider is given the document with the announced password, never the address.
    let job: rd_core::RemoteJobId = answer["job"]["id"]
        .as_str()
        .expect("job id")
        .parse()
        .expect("a job id");
    let source = harness
        .database
        .remote_job_source(job)
        .await
        .expect("source")
        .expect("the job keeps its source");
    let text = String::from_utf8(source).expect("UTF-8");
    assert!(text.contains("msg-1@example"), "{text}");
    assert!(
        text.contains("<meta type=\"password\">hit-secret</meta>"),
        "{text}"
    );
    assert!(
        !text.contains("SECRET"),
        "the indexer's key reached the job: {text}"
    );

    let states_now = states(&harness, &subscription).await;
    assert_eq!(states_now[first.id.to_string()], "queued", "{states_now}");
    assert_eq!(states_now[again.id.to_string()], "pending", "{states_now}");

    // The same release from another hit is the duplicate guard answering; the hit is decided.
    let (status, duplicate) = post_json(
        &harness.router,
        &hand_over_uri(again),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{duplicate}");
    assert_eq!(duplicate["already_running"], true);
    assert_eq!(duplicate["job"]["id"], answer["job"]["id"]);
    let states_now = states(&harness, &subscription).await;
    assert_eq!(states_now[again.id.to_string()], "queued", "{states_now}");

    // A decided hit is not sent twice.
    let (status, refused) = post_json(
        &harness.router,
        &hand_over_uri(first),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "subscription.remote_job_item_decided");
}

/// The indexer's refusal inside a `200` reaches the hit with its reason, and the hit stays.
#[tokio::test]
async fn an_indexer_refusal_keeps_the_hit_waiting_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    let torbox = account(&harness, "torbox").await;
    let asked = Arc::new(AtomicUsize::new(0));
    let server = indexer(asked.clone()).await;
    let (subscription, hits) = subscription_with_hits(
        &harness,
        &server,
        &[(
            "refused",
            "/getnzb-refused",
            rd_core::SubscriptionItemState::Pending,
        )],
    )
    .await;
    let hit = &hits[0];

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(hit),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{answer}");
    assert_eq!(answer["code"], "collector.nzb_rejected");
    assert!(
        answer["error"]
            .as_str()
            .is_some_and(|message| message.contains("Request limit reached")),
        "{answer}"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    let states = states(&harness, &subscription).await;
    assert_eq!(states[hit.id.to_string()], "pending", "{states}");
    let (status, jobs) = get_json(&harness.router, "/api/v1/remote-jobs").await;
    assert_eq!(status, StatusCode::OK, "{jobs}");
    assert_eq!(jobs, json!([]));
}
