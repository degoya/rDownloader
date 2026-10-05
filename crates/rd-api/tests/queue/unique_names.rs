//! A name a table keeps unique is a `409` with a stable code, never `internal.error`
//! (RD-1110-16): on creation and on a rename, for every object whose name the schema holds
//! unique — categories, storage roots, watched folders, NNTP servers and bandwidth profiles.
//! Notification targets are covered in the admin suite `notifications`.

use crate::common;

use axum::{Router, http::StatusCode};
use common::{post_json, put_json, test_router};
use serde_json::{Value, json};

/// Creates `first` and `second` at `collection`, then asks for a second `first` and for
/// `second` to be renamed to `first`; both must be refused with `code`.
async fn assert_name_is_unique(
    router: &Router,
    collection: &str,
    body: impl Fn(&str) -> Value,
    rename: impl Fn(&str) -> Value,
    code: &str,
) {
    let (status, _) = post_json(router, collection, body("first")).await;
    assert_eq!(status, StatusCode::CREATED, "{collection}");
    let (status, second) = post_json(router, collection, body("second")).await;
    assert_eq!(status, StatusCode::CREATED, "{collection}: {second}");
    let second = second["id"].as_str().expect("id").to_owned();

    let (status, refused) = post_json(router, collection, body("first")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{collection}: {refused}");
    assert_eq!(refused["code"], code, "{refused}");

    let (status, refused) =
        put_json(router, &format!("{collection}/{second}"), rename("first")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{collection}: {refused}");
    assert_eq!(refused["code"], code, "{refused}");
}

#[tokio::test]
async fn a_taken_category_or_storage_root_name_is_a_conflict() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let base = directory.path().to_path_buf();
    let root = |name: &str| {
        json!({
            "name": format!("unique-root-{name}"),
            "path": base.join(format!("unique-root-{name}")).to_string_lossy(),
            "is_default": false,
        })
    };
    assert_name_is_unique(
        &router,
        "/api/v1/storage-roots",
        root,
        root,
        "storage_root.name_or_path_taken",
    )
    .await;

    let (_, roots) = common::get_json(&router, "/api/v1/storage-roots").await;
    let root_id = roots[0]["id"].as_str().expect("root id").to_owned();
    let category = |name: &str| {
        json!({
            "name": format!("unique-category-{name}"),
            "color": "#336699",
            "storage_root_id": root_id,
            "relative_path": format!("unique-category-{name}"),
            "is_default": false,
        })
    };
    assert_name_is_unique(
        &router,
        "/api/v1/categories",
        category,
        category,
        "category.name_taken",
    )
    .await;
}

#[tokio::test]
async fn a_taken_hotfolder_server_or_profile_name_is_a_conflict() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let base = directory.path().to_path_buf();
    let hotfolder = |name: &str| {
        json!({
            "name": format!("unique-watch-{name}"),
            "executor": { "kind": "daemon" },
            "path": base.join(format!("unique-watch-{name}")).display().to_string(),
            "recursive": false,
            "category_id": null,
            "import_mode": "review",
            "processed_path": "processed",
            "failed_path": "failed",
            "enabled": false,
        })
    };
    assert_name_is_unique(
        &router,
        "/api/v1/hotfolders",
        hotfolder,
        hotfolder,
        "hotfolder.name_or_path_taken",
    )
    .await;

    let server = |name: &str| {
        json!({
            "name": format!("unique-news-{name}"),
            "host": "news.unique.test",
            "port": 563,
            "tls": true,
            "username": null,
            "password": null,
            "proxy_profile_id": null,
            "priority": 0,
            "max_connections": 2,
            "enabled": false,
        })
    };
    let server_rename = |name: &str| {
        let mut body = server(name);
        body["clear_password"] = json!(false);
        body
    };
    assert_name_is_unique(
        &router,
        "/api/v1/usenet/servers",
        server,
        server_rename,
        "usenet.server_name_taken",
    )
    .await;

    let profile = |name: &str| json!({ "name": format!("unique-profile-{name}") });
    assert_name_is_unique(
        &router,
        "/api/v1/bandwidth/profiles",
        profile,
        profile,
        "bandwidth.profile_name_taken",
    )
    .await;
}
