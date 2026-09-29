//! A storage root may not reach a directory the service runs things from (security review
//! 2026-09-28, finding 4) however it arrives: a settings import and a full restore refuse it
//! like create and update do (`crates/rd-api/tests/queue/storage_roots.rs`), with the same
//! `storage_root.protected_directory`, and change nothing.

use axum::http::StatusCode;
use rd_backup::restore::cutover::Layout;
use serde_json::json;

use crate::common;
use crate::full_restore::{PASSPHRASE, backed_up};

#[tokio::test]
async fn a_settings_import_may_not_bring_a_root_onto_a_protected_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let base = directory.path();
    let harness = common::test_harness(base).await;
    let downloads = base.join("downloads");
    harness
        .database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Downloads".to_owned(),
                path: downloads.display().to_string(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let (status, bundle) = common::post_json(
        &harness.router,
        "/api/v1/settings/export",
        json!({ "include_secrets": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bundle}");

    // The harness keeps its scripts in `<dir>/scripts`; a bundle may also name its own scripts
    // directory, which is the service's once the import is in.
    let hooks = base.join("hooks");
    for (root, scripts, kind) in [
        (base.join("scripts/inbox"), None, "scripts"),
        (base.to_path_buf(), None, "scripts"),
        (hooks.join("inbox"), Some(&hooks), "scripts"),
    ] {
        let mut refused = bundle.clone();
        refused["storage_roots"][0]["path"] = json!(root.display().to_string());
        if let Some(scripts) = scripts {
            refused["settings"]["scripts_directory"] = json!(scripts.display().to_string());
        }
        let (status, body) = common::post_json(
            &harness.router,
            "/api/v1/settings/import",
            json!({ "bundle": refused }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{}: {body}",
            root.display()
        );
        assert_eq!(body["code"], "storage_root.protected_directory", "{body}");
        assert_eq!(body["params"]["directory"], kind, "{body}");
    }
    let roots = harness.database.list_storage_roots().await.expect("roots");
    assert_eq!(roots.len(), 1);
    assert_eq!(
        roots[0].path,
        downloads.display().to_string(),
        "nothing replaced"
    );

    // The bundle as exported passes: the check refuses the root, not the import.
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/settings/import",
        json!({ "bundle": bundle }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_restore_may_not_map_a_root_onto_a_protected_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let base = directory.path();
    let harness = common::test_harness(base).await;
    let (run_id, _, root_id) = backed_up(&harness, base).await;
    let layout = Layout::new(harness.database.path());

    // The harness loads its plugins from `<dir>/plugins`.
    for step in ["/api/v1/backups/restore/test", "/api/v1/backups/restore"] {
        let (status, body) = common::post_json(
            &harness.router,
            step,
            json!({
                "source": { "run_id": run_id },
                "passphrase": PASSPHRASE,
                "mappings": [{
                    "storage_root_id": root_id,
                    "path": base.join("plugins/library").display().to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{step}: {body}");
        assert_eq!(body["code"], "storage_root.protected_directory", "{body}");
        assert_eq!(body["params"]["directory"], "plugins", "{body}");
    }
    let (_, status_body) = common::get_json(&harness.router, "/api/v1/backups/restore").await;
    assert_eq!(status_body["state"], "none", "nothing was staged");
    assert_eq!(
        std::fs::read_dir(layout.work()).map_or(0, Iterator::count),
        0,
        "no copy stayed behind"
    );
}
