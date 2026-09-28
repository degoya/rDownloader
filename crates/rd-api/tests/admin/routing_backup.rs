use crate::common;

use axum::{Router, http::StatusCode};
use common::{Harness, test_harness};

async fn seed_routing(harness: &Harness, directory: &std::path::Path) -> rd_core::CategoryId {
    let root = harness
        .database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Primary".to_owned(),
                path: directory.join("downloads").to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    let category = harness
        .database
        .create_category(rd_db::NewCategory {
            name: "Movies".to_owned(),
            color: "#336699".to_owned(),
            storage_root_id: root.id,
            relative_path: "movies".to_owned(),
            is_default: true,
            postprocess_level: Some(rd_core::PostprocessLevel::Unpack),
            script: None,
            cleanup_extensions: Some(vec!["nfo".to_owned()]),
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    harness
        .database
        .create_category_rule(rd_db::NewCategoryRule {
            name: "Movie files".to_owned(),
            priority: 10,
            source: Some(rd_core::IngressSource::Manual),
            domain: None,
            protocol: None,
            extension: Some("mkv".to_owned()),
            mime_type: None,
            name_regex: None,
            category_id: category.id,
            enabled: true,
        })
        .await
        .expect("rule");
    category.id
}

async fn export(router: &Router) -> serde_json::Value {
    let (status, bundle) = common::get_json(router, "/api/v1/routing/export").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        bundle.is_object(),
        "the export is a JSON document: {bundle}"
    );
    bundle
}

async fn import(router: &Router, bundle: serde_json::Value) -> (StatusCode, serde_json::Value) {
    common::post_json_strict(router, "/api/v1/routing/import", bundle).await
}

#[tokio::test]
async fn export_resolves_names_and_reimport_is_idempotent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed_routing(&harness, directory.path()).await;
    let bundle = export(&harness.router).await;
    assert_eq!(bundle["format"], "rdownloader-routing-bundle");
    assert_eq!(bundle["categories"][0]["storage_root_name"], "Primary");
    assert_eq!(bundle["rules"][0]["category_name"], "Movies");

    let (status, summary) = import(&harness.router, bundle).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["categories_created"], 0);
    assert_eq!(summary["categories_skipped"], 1);
    assert_eq!(summary["rules_created"], 0);
    assert_eq!(summary["rules_skipped"], 1);
    assert_eq!(
        harness
            .database
            .list_categories()
            .await
            .expect("categories")
            .len(),
        1
    );
}

/// Each part exports on its own; without `part` the export carries both, as it always did.
#[tokio::test]
async fn categories_and_rules_export_on_their_own() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed_routing(&harness, directory.path()).await;

    let (status, categories) =
        common::get_json(&harness.router, "/api/v1/routing/export?part=categories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(categories["categories"][0]["name"], "Movies");
    assert_eq!(categories["rules"], serde_json::json!([]));

    let (status, rules) =
        common::get_json(&harness.router, "/api/v1/routing/export?part=rules").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rules["categories"], serde_json::json!([]));
    assert_eq!(rules["rules"][0]["category_name"], "Movies");

    let both = export(&harness.router).await;
    assert_eq!(both["categories"].as_array().map(Vec::len), Some(1));
    assert_eq!(both["rules"].as_array().map(Vec::len), Some(1));

    // A rules-only file imports against categories that exist by name on the target.
    let (status, summary) = import(&harness.router, rules).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["categories_created"], 0);
    assert_eq!(
        summary["rules_skipped"], 1,
        "the same rule is already there"
    );

    let (status, _) =
        common::get_json(&harness.router, "/api/v1/routing/export?part=everything").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_merges_new_entries_and_keeps_existing_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed_routing(&harness, directory.path()).await;
    let mut bundle = export(&harness.router).await;
    bundle["categories"]
        .as_array_mut()
        .expect("categories")
        .push(serde_json::json!({
            "name": "Series",
            "color": "#00AA00",
            "storage_root_name": "Primary",
            "relative_path": "series",
            "is_default": true
        }));
    bundle["rules"]
        .as_array_mut()
        .expect("rules")
        .push(serde_json::json!({
            "name": "Series rule",
            "priority": 20,
            "extension": "mp4",
            "category_name": "Series",
            "enabled": true
        }));

    let (status, summary) = import(&harness.router, bundle).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["categories_created"], 1);
    assert_eq!(summary["categories_skipped"], 1);
    assert_eq!(summary["rules_created"], 1);
    assert_eq!(summary["rules_skipped"], 1);

    let categories = harness
        .database
        .list_categories()
        .await
        .expect("categories");
    let series = categories
        .iter()
        .find(|category| category.name == "Series")
        .expect("imported category");
    // The target already has a default category; the bundle must not displace it.
    assert!(!series.is_default);
    assert!(
        categories
            .iter()
            .find(|category| category.name == "Movies")
            .expect("existing category")
            .is_default
    );
    let rules = harness.database.list_category_rules().await.expect("rules");
    let series_rule = rules
        .iter()
        .find(|rule| rule.name == "Series rule")
        .expect("imported rule");
    assert_eq!(series_rule.category_id, series.id);
}

#[tokio::test]
async fn missing_storage_root_skips_category_and_dependent_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed_routing(&harness, directory.path()).await;
    let bundle = serde_json::json!({
        "format": "rdownloader-routing-bundle",
        "version": 1,
        "exported_at": "2026-01-01T00:00:00Z",
        "app_version": "0.0.0",
        "categories": [{
            "name": "Orphan",
            "color": "#112233",
            "storage_root_name": "Nope",
            "relative_path": "orphan",
            "is_default": false
        }],
        "rules": [{
            "name": "Orphan rule",
            "priority": 1,
            "category_name": "Orphan",
            "enabled": true
        }]
    });
    let (status, summary) = import(&harness.router, bundle).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["categories_created"], 0);
    assert_eq!(summary["categories_skipped"], 1);
    assert_eq!(summary["rules_created"], 0);
    assert_eq!(summary["rules_skipped"], 1);
}

#[tokio::test]
async fn invalid_format_and_version_are_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let base = serde_json::json!({
        "format": "rdownloader-routing-bundle",
        "version": 1,
        "exported_at": "2026-01-01T00:00:00Z",
        "app_version": "0.0.0",
        "categories": [],
        "rules": []
    });
    let mut wrong_format = base.clone();
    wrong_format["format"] = serde_json::json!("something-else");
    let (status, error) = import(&harness.router, wrong_format).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "routing.backup_invalid");

    let mut wrong_version = base;
    wrong_version["version"] = serde_json::json!(999);
    let (status, error) = import(&harness.router, wrong_version).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "routing.backup_version_unsupported");
}
