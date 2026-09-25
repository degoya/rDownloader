use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

struct Harness {
    router: Router,
    database: rd_db::Database,
}

async fn test_harness(directory: &std::path::Path) -> Harness {
    let database = rd_db::Database::open(directory.join("routing-backup.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );
    let media_settings = rd_media::shared_settings(&database)
        .await
        .expect("media settings");
    let (_media_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery settings");
    let stream_settings = rd_stream::shared_settings(&database)
        .await
        .expect("stream settings");
    let torrent_settings = rd_torrent::shared_settings(&database)
        .await
        .expect("torrent settings");
    let torrent = rd_torrent::TorrentService::start(
        database.clone(),
        torrent_settings.clone(),
        directory.to_path_buf(),
        directory.join("downloads"),
    );
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: std::time::Duration::from_secs(60),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    let remote = rd_api::RemoteServices::new(
        database.clone(),
        secrets.clone(),
        std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
        rd_http::SharedNetworkDefaults::default(),
    );
    let state = rd_api::AppState::new(
        database.clone(),
        scheduler,
        secrets,
        plugins,
        extraction,
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent,
        torrent_settings,
        rd_power::PowerService::default(),
        rd_core::PostprocessHold::new(),
        remote,
    );
    state.auth.set_disabled(true);
    Harness {
        router: rd_api::router(state),
        database,
    }
}

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
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/routing/export")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

async fn import(router: &Router, bundle: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/routing/import")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bundle.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).expect("JSON response");
    (status, json)
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
