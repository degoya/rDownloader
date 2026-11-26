//! Changing and deleting a watched folder (audit 2026-11-25, TST-7). The poll interval has its
//! suite (`hotfolder_poll`), the unique names theirs (`unique_names`).

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_router};
use serde_json::{Value, json};

fn folder(name: &str, path: &std::path::Path) -> Value {
    json!({
        "name": name,
        "executor": { "kind": "daemon" },
        "path": path.display().to_string(),
        "recursive": false,
        "category_id": null,
        "import_mode": "review",
        "processed_path": "processed",
        "failed_path": "failed",
        "enabled": false,
    })
}

async fn names(router: &axum::Router) -> Vec<String> {
    let (status, body) = get_json(router, "/api/v1/hotfolders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .expect("a list")
        .iter()
        .map(|entry| entry["name"].as_str().expect("a name").to_owned())
        .collect()
}

#[tokio::test]
async fn a_folder_is_changed_and_deleted_by_its_id() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let watched = directory.path().join("watched");
    let (status, created) = post_json(
        &router,
        "/api/v1/hotfolders",
        folder("inbox", &watched.join("first")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("an id").to_owned();
    let uri = format!("/api/v1/hotfolders/{id}");

    let mut changed = folder("inbox renamed", &watched.join("second"));
    changed["recursive"] = json!(true);
    changed["import_mode"] = json!("enqueue");
    let (status, body) = put_json(&router, &uri, changed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id.as_str(), "{body}");
    assert_eq!(body["recursive"], true, "{body}");
    assert_eq!(body["import_mode"], "enqueue", "{body}");
    assert_eq!(names(&router).await, ["inbox renamed"]);

    let (status, body) = delete_json(&router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "hotfolder.deleted", "{body}");
    assert!(names(&router).await.is_empty());

    for (status, body) in [
        delete_json(&router, &uri).await,
        put_json(&router, &uri, folder("ghost", &watched.join("third"))).await,
    ] {
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["code"], "hotfolder.not_found", "{body}");
    }
}
