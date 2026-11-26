//! `GET <collection>/{id}` (RD-1480-10): one row exactly as its list shows it, a coded `404` for an
//! id nobody has, and the `Location` of a create.

use crate::common;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use common::{Harness, WAIT, eventually, get_json, parked_harness, patch_json, post_json};
use serde_json::{Value, json};
use sqlx::Connection;

/// Every single read of an editable resource, and the code of its `404`.
const SINGLE_READS: &[(&str, &str)] = &[
    ("/api/v1/accounts", "account.not_found"),
    ("/api/v1/api-tokens", "api.token_not_found"),
    ("/api/v1/auth-profiles", "authprofile.not_found"),
    ("/api/v1/automations", "automation.not_found"),
    (
        "/api/v1/backups/destinations",
        "backup.destination_not_found",
    ),
    ("/api/v1/bandwidth/profiles", "bandwidth.profile_not_found"),
    ("/api/v1/categories", "category.not_found"),
    ("/api/v1/category-rules", "category_rule.not_found"),
    (
        "/api/v1/collector/candidates",
        "collector.candidate_not_found",
    ),
    ("/api/v1/collector/packages", "package.not_found"),
    ("/api/v1/downloads", "download.not_found"),
    ("/api/v1/hotfolders", "hotfolder.not_found"),
    ("/api/v1/indexers", "indexer.not_found"),
    ("/api/v1/link-filters", "link_filter.not_found"),
    ("/api/v1/notifications/rules", "notification.rule_not_found"),
    (
        "/api/v1/notifications/targets",
        "notification.target_not_found",
    ),
    ("/api/v1/nzb/imports", "nzb.import_not_found"),
    (
        "/api/v1/object-storage/profiles",
        "object_storage.profile_not_found",
    ),
    ("/api/v1/packages", "package.not_found"),
    ("/api/v1/plugins", "plugin.not_installed"),
    (
        "/api/v1/plugins/repositories",
        "plugin_repository.not_found",
    ),
    ("/api/v1/proxy-profiles", "proxy.not_found"),
    ("/api/v1/remote-credentials", "remote.credential_not_found"),
    ("/api/v1/site-rules", "site_rules.not_found"),
    ("/api/v1/storage-roots", "storage_root.not_found"),
    ("/api/v1/streams/channels", "stream.channel_not_found"),
    ("/api/v1/streams/schedules", "stream.schedule_not_found"),
    ("/api/v1/subscriptions", "subscription.not_found"),
    ("/api/v1/usenet/servers", "usenet.server_not_found"),
];

/// An id of the right shape that no row has.
const NOBODY: &str = "00000000-0000-7000-8000-000000000000";

/// `POST`, with the answer's headers.
async fn create(harness: &Harness, uri: &str, body: Value) -> (HeaderMap, Value) {
    let request = common::request_to("POST", uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let (status, headers, bytes) = common::send_raw(&harness.router, request).await;
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(status, StatusCode::CREATED, "{uri}: {body}");
    (headers, body)
}

/// The row of the list `uri` with this id.
async fn listed(harness: &Harness, uri: &str, id: &str) -> Value {
    let (status, rows) = get_json(&harness.router, uri).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    rows.as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("{uri} does not list {id}"))
}

#[tokio::test]
async fn an_id_nobody_has_is_a_coded_404() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    for (collection, code) in SINGLE_READS {
        let uri = format!("{collection}/{NOBODY}");
        let (status, body) = get_json(&harness.router, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert_eq!(body["code"], *code, "{uri}");
    }
}

/// What one row costs is what the list of them costs, never less.
#[test]
fn a_single_read_costs_what_its_list_costs() {
    for (collection, _) in SINGLE_READS {
        // The destinations of the full backup are listed in its configuration.
        let list = if *collection == "/api/v1/backups/destinations" {
            "/api/v1/backups"
        } else {
            collection
        };
        assert_eq!(
            rd_api::required_scope(&format!("{collection}/{{id}}"), "GET"),
            rd_api::required_scope(list, "GET"),
            "{collection}"
        );
    }
}

#[tokio::test]
async fn a_malformed_id_is_refused_with_a_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (status, body) = get_json(&harness.router, "/api/v1/downloads/not-an-id").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "request.invalid_id");
}

#[tokio::test]
async fn a_created_download_is_read_where_its_location_says() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (headers, created) = create(
        &harness,
        "/api/v1/downloads",
        json!({ "url": "http://example.test/a.bin", "paused": true }),
    )
    .await;
    let id = created["id"].as_str().expect("id");
    let location = headers[header::LOCATION].to_str().expect("text");
    assert_eq!(location, format!("/api/v1/downloads/{id}"));
    let (status, one) = get_json(&harness.router, location).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one, listed(&harness, "/api/v1/downloads", id).await);
}

/// The package's archive password is what the list shows, too (RD-104-04).
#[tokio::test]
async fn a_package_reads_as_the_list_shows_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let (_, created) = create(
        &harness,
        "/api/v1/downloads",
        json!({ "url": "http://example.test/a.bin", "paused": true }),
    )
    .await;
    let uri = format!(
        "/api/v1/packages/{}",
        created["package_id"].as_str().expect("package")
    );
    let (status, body) = patch_json(
        &harness.router,
        &uri,
        json!({ "password": "archive-password" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, one) = get_json(&harness.router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["password"], "archive-password");
    let id = one["id"].as_str().expect("id");
    assert_eq!(one, listed(&harness, "/api/v1/packages", id).await);
}

/// Configuration rows: the same row as in the list, without the secret the create took in.
#[tokio::test]
async fn a_configuration_row_reads_as_the_list_shows_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    for (uri, body) in [
        (
            "/api/v1/proxy-profiles",
            json!({ "name": "single", "kind": "socks5", "endpoint": "socks5h://127.0.0.1:1080",
                "username": "reader", "password": "never-answered" }),
        ),
        (
            "/api/v1/remote-credentials",
            json!({ "name": "single", "protocol": "ftp", "host": "127.0.0.1", "port": 21,
                "username": "reader", "auth_mode": "password", "secret": "never-answered" }),
        ),
        (
            "/api/v1/auth-profiles",
            json!({ "name": "single", "scope": "files.single-read.test",
                "include_subdomains": false, "method": "bearer", "username": null,
                "secret": "never-answered", "certificate_pem": null, "expires_at": null,
                "enabled": true }),
        ),
    ] {
        let (headers, created) = create(&harness, uri, body).await;
        let id = created["id"].as_str().expect("id");
        let location = headers[header::LOCATION].to_str().expect("text");
        assert_eq!(location, format!("{uri}/{id}"));
        let (status, one) = get_json(&harness.router, location).await;
        assert_eq!(status, StatusCode::OK, "{one}");
        assert_eq!(one, listed(&harness, uri, id).await);
        assert!(!one.to_string().contains("never-answered"), "{one}");
    }
}

/// A link of the LinkGrabber and its package, and neither once the link is queued: the list
/// hides them then, and a read by id does not bring them back.
#[tokio::test]
async fn a_queued_link_leaves_the_linkgrabber_here_as_well() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    // A server that answers at once: a closed port is retried for longer than `WAIT` on Windows.
    // Entered by hand, so a loopback address may be asked.
    let server = rd_testkit::serve(axum::Router::new()).await;
    let (status, batch) = post_json(
        &harness.router,
        "/api/v1/collector/batches",
        json!({ "text": server.url("/one.bin"), "source": "manual" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{batch}");
    // The check writes the link's state, so it has to be over before the test writes it.
    let link = eventually(WAIT, "the link was checked", || async {
        let (_, links) = get_json(&harness.router, "/api/v1/collector/candidates").await;
        let link = &links[0];
        (link["state"] != "checking" && !link["checked_at"].is_null())
            .then(|| link["id"].as_str().map(ToOwned::to_owned))
            .flatten()
    })
    .await;
    let link_uri = format!("/api/v1/collector/candidates/{link}");
    let (status, one) = get_json(&harness.router, &link_uri).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["id"], link.as_str());
    let package = one["package_id"].as_str().expect("package").to_owned();
    let package_uri = format!("/api/v1/collector/packages/{package}");
    let (status, one) = get_json(&harness.router, &package_uri).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(
        one,
        listed(&harness, "/api/v1/collector/packages", &package).await
    );

    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", harness.database_path.display()))
            .await
            .expect("connect");
    sqlx::query("UPDATE link_candidates SET state = 'enqueued' WHERE id = ?")
        .bind(&link)
        .execute(&mut connection)
        .await
        .expect("queue the link");
    for uri in [link_uri, package_uri] {
        let (status, body) = get_json(&harness.router, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
}
