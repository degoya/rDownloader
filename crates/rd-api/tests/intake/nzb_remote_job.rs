//! RD-191-13: an NZB from the LinkGrabber handed to a remote-job provider instead of the queue.
//!
//! The server writes the import back out as an NZB and submits it as a container through the
//! same path `POST /api/v1/accounts/{id}/remote-jobs` takes; the import stays in the list,
//! marked with the job. The refusals that need no plugin are checked without components; the
//! ones that read what a plugin declares, and the hand-over itself, install the real
//! `torbox-jobs` (takes NZBs) and `realdebrid-torrents` (does not) and are named
//! `on_real_components`, so the `no-components` profile leaves them out.
//!
//! The Downloads view hands over the NZB behind a queued package the same way
//! (`POST /api/v1/packages/{id}/remote-job`), where the import route says `nzb.already_enqueued`.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness};
use serde_json::json;

/// The development key the harness's verifier accepts unsigned packages under.
const DEV_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

/// A bundled plugin with its built component, unsigned, under the development key.
fn package(directory: &str) -> Vec<u8> {
    let crate_root =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR at run time");
    let root = std::path::Path::new(&crate_root)
        .join("../../plugins")
        .join(directory);
    let manifest = std::fs::read_to_string(root.join("manifest.toml"))
        .expect("the bundled manifest")
        .lines()
        .map(|line| {
            if line.starts_with("key_id = ") {
                "key_id = \"dev\"".to_owned()
            } else if line.starts_with("public_key = ") {
                format!("public_key = \"{DEV_PUBLIC_KEY}\"")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut locales: Vec<(String, Vec<u8>)> = std::fs::read_dir(root.join("locales"))
        .expect("the plugin's locales")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let language = name.strip_suffix(".json")?.to_owned();
            Some((language, std::fs::read(entry.path()).ok()?))
        })
        .collect();
    locales.sort_by(|left, right| left.0.cmp(&right.0));
    rd_plugin_host::package_plugin(
        manifest.as_bytes(),
        &rd_plugin_host::artifact::component(&format!("rd-plugin-{directory}")),
        &locales,
        None,
    )
    .expect("package")
}

async fn install(harness: &common::Harness, directory: &str) {
    harness
        .state
        .plugins
        .install_bytes(package(directory))
        .await
        .expect("install");
}

async fn account(harness: &common::Harness, provider: &str) -> rd_core::AccountId {
    harness
        .database
        .create_account(rd_db::NewAccount {
            provider: provider.to_owned(),
            label: format!("{provider} account"),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

/// Two files, one with two articles, and an archive password.
fn nzb_import(name: &str, digest: &str) -> rd_db::NewNzbImport {
    let file = |subject: &str, articles: &[(u32, &str)]| rd_db::NewNzbFile {
        subject: subject.to_owned(),
        poster: "poster <poster@example.test>".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: articles
            .iter()
            .map(|(number, message_id)| rd_db::NewNzbSegment {
                number: *number,
                bytes: 4096,
                message_id: (*message_id).to_owned(),
            })
            .collect(),
    };
    rd_db::NewNzbImport {
        name: name.to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: rd_core::ImportMode::Review,
        source: rd_core::IngressSource::Manual,
        source_path: None,
        password: Some("secret-archive".to_owned()),
        announce_arrival: false,
        files: vec![
            file(
                "\"Show.S01E01.rar\" yEnc (1/2)",
                &[(1, "rar-1@example.test"), (2, "rar-2@example.test")],
            ),
            file(
                "\"Show.S01E01.par2\" yEnc (1/1)",
                &[(1, "par-1@example.test")],
            ),
        ],
    }
}

fn hand_over_uri(import: &rd_core::NzbImport) -> String {
    format!("/api/v1/nzb/imports/{}/remote-job", import.id)
}

fn package_uri(package: &str) -> String {
    format!("/api/v1/packages/{package}/remote-job")
}

/// The refusals that come before any plugin is asked: an import that is not in the LinkGrabber
/// to hand over, and an account that does not exist or runs no remote jobs.
#[tokio::test]
async fn an_nzb_import_that_cannot_go_is_refused_before_anything_is_sent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let torbox = account(&harness, "torbox").await;

    let missing = format!(
        "/api/v1/nzb/imports/{}/remote-job",
        rd_core::NzbImportId::new()
    );
    let (status, answer) =
        post_json(&harness.router, &missing, json!({ "account_id": torbox })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "nzb.import_not_found");

    let failed = harness
        .database
        .record_nzb_import_failure(rd_db::FailedNzbImport {
            name: "broken.nzb".to_owned(),
            sha256: "f0".repeat(32),
            source_path: None,
            error: "NZB contains no files".to_owned(),
        })
        .await
        .expect("failed import");
    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&failed),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_import_failed");

    let queued = harness
        .database
        .add_nzb_import(nzb_import("queued.nzb", "a2"))
        .await
        .expect("import");
    let (status, answer) = post_json(
        &harness.router,
        &format!("/api/v1/nzb/imports/{}/enqueue", queued.id),
        json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let queued_package = answer["id"].as_str().expect("package id").to_owned();
    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&queued),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "nzb.already_enqueued");

    // From the Downloads view the queued NZB is no refusal: its package goes on to the account
    // checks, which refuse here because no remote-job plugin is installed.
    let (status, answer) = post_json(
        &harness.router,
        &package_uri(&queued_package),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_not_remote_account");

    let (status, answer) = post_json(
        &harness.router,
        &package_uri(&rd_core::PackageId::new().to_string()),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "package.not_found");

    let plain = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Plain links".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let (status, answer) = post_json(
        &harness.router,
        &package_uri(&plain.id.to_string()),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "package.remote_job_no_nzb");

    let waiting = harness
        .database
        .add_nzb_import(nzb_import("waiting.nzb", "a3"))
        .await
        .expect("import");
    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&waiting),
        json!({ "account_id": rd_core::AccountId::new() }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "remote_job.no_account");

    // No remote-job plugin is installed, so no account runs remote jobs.
    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&waiting),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_not_remote_account");

    let (status, imports) = get_json(&harness.router, "/api/v1/nzb/imports").await;
    assert_eq!(status, StatusCode::OK, "{imports}");
    assert!(
        imports
            .as_array()
            .expect("imports")
            .iter()
            .all(|import| import["handed_over"].is_null()),
        "a refusal marks nothing: {imports}"
    );
}

/// The owner's request: the NZB goes to TorBox as a container named after the import, the
/// import stays in the LinkGrabber marked with the job, and a second press is the same job.
#[tokio::test]
async fn an_nzb_import_reaches_a_provider_that_takes_nzbs_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    install(&harness, "realdebrid-torrents").await;
    let torbox = account(&harness, "torbox").await;
    let realdebrid = account(&harness, "realdebrid").await;
    let import = harness
        .database
        .add_nzb_import(nzb_import("Show.S01E01.nzb", "b1"))
        .await
        .expect("import");

    // What the LinkGrabber offers: providers that take NZBs, not every remote-job provider.
    let (status, all) = get_json(&harness.router, "/api/v1/remote-jobs/providers").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all, json!(["realdebrid", "torbox"]));
    let (status, nzb) = get_json(
        &harness.router,
        "/api/v1/remote-jobs/providers?container=nzb",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{nzb}");
    assert_eq!(nzb, json!(["torbox"]));

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&import),
        json!({ "account_id": realdebrid }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "nzb.remote_job_no_nzb");

    let (status, answer) = post_json(
        &harness.router,
        &hand_over_uri(&import),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["already_running"], false);
    assert_eq!(answer["job"]["source_kind"], "container");
    assert_eq!(answer["job"]["source_name"], "Show.S01E01.nzb");
    assert_eq!(answer["job"]["account_id"], torbox.to_string());
    assert_eq!(
        answer["import"]["handed_over"]["account_id"],
        torbox.to_string()
    );
    assert_eq!(
        answer["import"]["handed_over"]["remote_job_id"],
        answer["job"]["id"]
    );

    // The bytes the sweep will hand TorBox are the import, written back out.
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
    for expected in [
        "<nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\">",
        "<meta type=\"name\">Show.S01E01</meta>",
        "<meta type=\"password\">secret-archive</meta>",
        "poster=\"poster &lt;poster@example.test&gt;\"",
        "<group>alt.binaries.test</group>",
        "<segment bytes=\"4096\" number=\"2\">rar-2@example.test</segment>",
        "<segment bytes=\"4096\" number=\"1\">par-1@example.test</segment>",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }

    // Still in the LinkGrabber, marked.
    let (status, imports) = get_json(&harness.router, "/api/v1/nzb/imports").await;
    assert_eq!(status, StatusCode::OK, "{imports}");
    let listed = imports
        .as_array()
        .expect("imports")
        .iter()
        .find(|entry| entry["id"] == import.id.to_string())
        .expect("the import stays listed");
    assert_eq!(listed["state"], "imported");
    assert_eq!(listed["handed_over"]["remote_job_id"], answer["job"]["id"]);

    // A second press is the duplicate guard answering, not a second job.
    let (status, again) = post_json(
        &harness.router,
        &hand_over_uri(&import),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["already_running"], true);
    assert_eq!(again["job"]["id"], answer["job"]["id"]);

    // Removing the job from the list clears the mark; the NZB is the person's again.
    let (status, removed) =
        common::delete_json(&harness.router, &format!("/api/v1/remote-jobs/{job}")).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    let (_, imports) = get_json(&harness.router, "/api/v1/nzb/imports").await;
    let listed = imports
        .as_array()
        .expect("imports")
        .iter()
        .find(|entry| entry["id"] == import.id.to_string())
        .expect("the import stays listed");
    assert!(listed["handed_over"].is_null(), "{listed}");
}

/// The owner's extension: the NZB behind a queued package goes from the Downloads view, the
/// package stays, and its import carries the mark the Downloads view shows.
#[tokio::test]
async fn a_queued_nzb_package_reaches_a_provider_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install(&harness, "torbox-jobs").await;
    let torbox = account(&harness, "torbox").await;
    let import = harness
        .database
        .add_nzb_import(nzb_import("Show.S01E02.nzb", "c1"))
        .await
        .expect("import");
    let (status, package) = post_json(
        &harness.router,
        &format!("/api/v1/nzb/imports/{}/enqueue", import.id),
        json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{package}");
    let package_id = package["id"].as_str().expect("package id").to_owned();

    let (status, answer) = post_json(
        &harness.router,
        &package_uri(&package_id),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["already_running"], false);
    assert_eq!(answer["job"]["source_kind"], "container");
    assert_eq!(answer["job"]["source_name"], "Show.S01E02.nzb");
    assert_eq!(answer["import"]["id"], import.id.to_string());
    assert_eq!(answer["import"]["state"], "enqueued");
    assert_eq!(
        answer["import"]["handed_over"]["remote_job_id"],
        answer["job"]["id"]
    );

    // The package stays in the download list.
    let (status, packages) = get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    assert!(
        packages
            .as_array()
            .expect("packages")
            .iter()
            .any(|entry| entry["id"] == package_id.as_str()),
        "{packages}"
    );

    let (status, again) = post_json(
        &harness.router,
        &package_uri(&package_id),
        json!({ "account_id": torbox }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["already_running"], true);
    assert_eq!(again["job"]["id"], answer["job"]["id"]);
}
