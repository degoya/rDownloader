//! RD-1440-07: a LinkGrabber package handed to a remote-job provider instead of the queue.
//!
//! Every link becomes one remote-job source through the duplicate guard every remote job passes.
//! The setting `linkgrabber_hand_over_remove_links` (on by default) decides whether the links
//! handed over leave the LinkGrabber -- a package left empty with them -- or stay marked with
//! their job; what could not go stays either way, named with its code. The refusals that need no
//! plugin run without components; the hand-overs install the real `torbox-jobs` (magnets,
//! addresses, torrents) and `realdebrid-torrents` (magnets and torrents only) and are named
//! `on_real_components`, so the `no-components` profile leaves them out.

use crate::common;
use crate::nzb_remote_job::{account, install};

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_harness};
use serde_json::{Value, json};

/// A magnet TorBox and Real-Debrid both identify by its info hash.
fn magnet(hash_digit: char, name: &str) -> String {
    let hash: String = std::iter::repeat_n(hash_digit, 40).collect();
    format!("magnet:?xt=urn:btih:{hash}&dn={name}")
}

/// One LinkGrabber package holding `urls`, pasted by hand; answers the package id.
async fn package(harness: &common::Harness, name: &str, urls: &[String]) -> String {
    let (_, packages, _) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some(name.to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: urls.iter().map(|url| url.parse().expect("url")).collect(),
            providers: vec![None; urls.len()],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1, "one package for {name}");
    packages[0].id.to_string()
}

fn uri(package: &str) -> String {
    format!("/api/v1/collector/packages/{package}/remote-job")
}

/// The LinkGrabber's links of one package, by address.
async fn links_of(harness: &common::Harness, package: &str) -> Vec<Value> {
    let (status, candidates) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    assert_eq!(status, StatusCode::OK, "{candidates}");
    candidates
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|candidate| candidate["package_id"] == package)
        .cloned()
        .collect()
}

async fn package_listed(harness: &common::Harness, package: &str) -> bool {
    let (status, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    packages
        .as_array()
        .expect("packages")
        .iter()
        .any(|entry| entry["id"] == package)
}

async fn job_count(harness: &common::Harness) -> usize {
    let (status, jobs) = get_json(&harness.router, "/api/v1/remote-jobs").await;
    assert_eq!(status, StatusCode::OK, "{jobs}");
    jobs.as_array().expect("jobs").len()
}

/// The addresses in one list of the answer.
fn urls_in(answer: &Value, list: &str, links: &[Value]) -> Vec<String> {
    answer[list]
        .as_array()
        .expect(list)
        .iter()
        .map(|entry| {
            links
                .iter()
                .find(|link| link["id"] == entry["candidate_id"])
                .and_then(|link| link["url"].as_str())
                .expect("a link of the package")
                .to_owned()
        })
        .collect()
}

/// The refusals that come before any plugin is asked; nothing is sent and nothing is removed.
#[tokio::test]
async fn a_package_that_cannot_go_is_refused_before_anything_is_sent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let torbox = account(&harness, "torbox").await;
    let release = package(
        &harness,
        "Refused.S01",
        &["https://files.example/Refused.S01E01.mkv".to_owned()],
    )
    .await;

    let missing = uri(&rd_core::CollectorPackageId::new().to_string());
    let (status, answer) =
        post_json(&harness.router, &missing, json!({ "account_id": torbox })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "package.not_found");

    let (status, answer) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": rd_core::AccountId::new() }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "remote_job.no_account");

    // No remote-job plugin is installed, so no account runs remote jobs.
    let (status, answer) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "collector.remote_job_not_remote_account");

    assert_eq!(links_of(&harness, &release).await.len(), 1);
    assert_eq!(job_count(&harness).await, 0);
}

/// The default: every link reaches TorBox, each leaves the LinkGrabber, and the package goes
/// with the last one.
#[tokio::test]
async fn a_package_handed_over_whole_leaves_the_linkgrabber_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    let torbox = account(&harness, "torbox").await;
    let urls = [
        magnet('a', "Whole.S01"),
        "https://files.example/Whole.S01E01.mkv".to_owned(),
    ];
    let release = package(&harness, "Whole.S01", &urls).await;

    let (status, answer) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["handed_over"].as_array().expect("handed").len(), 2);
    assert_eq!(answer["stayed"], json!([]), "{answer}");
    assert_eq!(answer["links_removed"], true);
    assert_eq!(answer["package_removed"], true);
    let kinds: Vec<&str> = answer["handed_over"]
        .as_array()
        .expect("handed")
        .iter()
        .map(|entry| entry["job"]["source_kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["magnet", "address"]);
    assert!(
        answer["handed_over"]
            .as_array()
            .expect("handed")
            .iter()
            .all(|entry| entry["already_running"] == false
                && entry["job"]["account_id"] == torbox.to_string())
    );

    assert!(links_of(&harness, &release).await.is_empty());
    assert!(!package_listed(&harness, &release).await);
    assert_eq!(job_count(&harness).await, 2);
}

/// Real-Debrid takes the magnet and not the address: the magnet goes -- counted as handed over
/// though the account already had its job -- and the address stays with its reason, holding the
/// package. A package of addresses only sends nothing and loses nothing.
#[tokio::test]
async fn what_the_provider_refuses_stays_with_its_reason_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "realdebrid-torrents").await;
    let realdebrid = account(&harness, "realdebrid").await;
    let torrent = magnet('b', "Part.S01");
    let urls = [
        torrent.clone(),
        "https://files.example/Part.S01E01.mkv".to_owned(),
    ];
    let release = package(&harness, "Part.S01", &urls).await;
    let links = links_of(&harness, &release).await;

    // The magnet was handed over by hand before: the duplicate guard answers, nothing is sent.
    let (status, earlier) = post_json(
        &harness.router,
        &format!("/api/v1/accounts/{realdebrid}/remote-jobs"),
        json!({ "magnet": torrent.clone() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{earlier}");

    let (status, answer) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": realdebrid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(urls_in(&answer, "handed_over", &links), [torrent.as_str()]);
    assert_eq!(answer["handed_over"][0]["already_running"], true);
    assert_eq!(answer["handed_over"][0]["job"]["id"], earlier["job"]["id"]);
    assert_eq!(
        urls_in(&answer, "stayed", &links),
        ["https://files.example/Part.S01E01.mkv"]
    );
    let code = answer["stayed"][0]["code"].as_str().expect("a code");
    assert!(!code.is_empty(), "{answer}");
    assert_eq!(answer["package_removed"], false);

    let left = links_of(&harness, &release).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["url"], "https://files.example/Part.S01E01.mkv");
    assert!(package_listed(&harness, &release).await);
    assert_eq!(job_count(&harness).await, 1, "no second job for the magnet");

    let addresses = package(
        &harness,
        "Nothing.S01",
        &[
            "https://files.example/Nothing.S01E01.mkv".to_owned(),
            "ftp://files.example/Nothing.S01E02.mkv".to_owned(),
        ],
    )
    .await;
    let (status, answer) = post_json(
        &harness.router,
        &uri(&addresses),
        json!({ "account_id": realdebrid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["handed_over"], json!([]));
    assert_eq!(answer["stayed"].as_array().expect("stayed").len(), 2);
    assert!(
        answer["stayed"]
            .as_array()
            .expect("stayed")
            .iter()
            .any(|entry| entry["code"] == "collector.remote_job_source_unsupported"),
        "the ftp link names why: {answer}"
    );
    assert_eq!(links_of(&harness, &addresses).await.len(), 2);
    assert!(package_listed(&harness, &addresses).await);
    assert_eq!(job_count(&harness).await, 1);
}

/// The setting off: every link stays, marked with its job, and a second hand-over reports those
/// jobs without sending anything again.
#[tokio::test]
async fn with_the_setting_off_the_links_stay_marked_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    let torbox = account(&harness, "torbox").await;
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(settings["linkgrabber_hand_over_remove_links"], true);
    assert_eq!(settings["linkgrabber_hand_over_enabled"], true);
    // As in `managed_tools`: the harness runs with the login off, and a saved document that
    // says otherwise would switch it on under the following requests.
    settings["admin_login_disabled"] = json!(true);
    settings["linkgrabber_hand_over_remove_links"] = json!(false);
    let (status, saved) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let urls = [
        magnet('c', "Kept.S01"),
        "https://files.example/Kept.S01E01.mkv".to_owned(),
    ];
    let release = package(&harness, "Kept.S01", &urls).await;
    let (status, first) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["handed_over"].as_array().expect("handed").len(), 2);
    assert_eq!(first["links_removed"], false);
    assert_eq!(first["package_removed"], false);

    let links = links_of(&harness, &release).await;
    assert_eq!(links.len(), 2, "every link stays");
    for entry in first["handed_over"].as_array().expect("handed") {
        let link = links
            .iter()
            .find(|link| link["id"] == entry["candidate_id"])
            .expect("the link stays");
        assert_eq!(link["handed_over"]["remote_job_id"], entry["job"]["id"]);
        assert_eq!(link["handed_over"]["account_id"], torbox.to_string());
    }
    assert_eq!(job_count(&harness).await, 2);

    let (status, again) = post_json(
        &harness.router,
        &uri(&release),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let jobs = |answer: &Value| -> Vec<Value> {
        answer["handed_over"]
            .as_array()
            .expect("handed")
            .iter()
            .map(|entry| entry["job"]["id"].clone())
            .collect()
    };
    assert_eq!(jobs(&again), jobs(&first));
    assert!(
        again["handed_over"]
            .as_array()
            .expect("handed")
            .iter()
            .all(|entry| entry["already_running"] == true)
    );
    assert_eq!(job_count(&harness).await, 2, "nothing was sent again");
    assert_eq!(links_of(&harness, &release).await.len(), 2);
}
