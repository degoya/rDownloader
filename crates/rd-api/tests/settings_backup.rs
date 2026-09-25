use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use secrecy::ExposeSecret;
use tower::ServiceExt;

struct Harness {
    router: Router,
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
}

/// Token seeded into an auth profile; a bundle without secrets must never contain it.
const AUTH_PROFILE_TOKEN: &str = "auth-profile-token-must-stay-out-of-backups";

struct SeededConfig {
    auth_profile_id: rd_core::AuthProfileId,
    root_id: rd_core::StorageRootId,
    category_id: rd_core::CategoryId,
    rule_id: rd_core::CategoryRuleId,
    hotfolder_id: rd_core::HotFolderId,
    stream_id: rd_core::StreamChannelId,
    proxy_id: rd_core::ProxyProfileId,
    account_id: rd_core::AccountId,
    server_id: rd_core::UsenetServerId,
    old_refs: Vec<String>,
}

async fn test_harness(directory: &std::path::Path) -> Harness {
    let database = rd_db::Database::open(directory.join("settings-backup.sqlite3"))
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
    let state = rd_api::AppState::new(
        database.clone(),
        scheduler,
        secrets.clone(),
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
        rd_api::RemoteServices::new(
            database.clone(),
            secrets.clone(),
            std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
            rd_http::SharedNetworkDefaults::default(),
        ),
    );
    state.auth.set_disabled(true);
    Harness {
        router: rd_api::router(state),
        database,
        secrets,
    }
}

async fn post(
    router: &Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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

async fn seed_all(harness: &Harness, directory: &std::path::Path) -> SeededConfig {
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
    let rule = harness
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
    let hotfolder = harness
        .database
        .create_hotfolder(rd_db::NewHotFolder {
            name: "Incoming".to_owned(),
            executor: rd_core::HotFolderExecutor::Daemon,
            path: directory.join("watch").to_string_lossy().into_owned(),
            recursive: true,
            category_id: Some(category.id),
            import_mode: rd_core::ImportMode::Review,
            processed_path: "processed".to_owned(),
            failed_path: "failed".to_owned(),
            enabled: false,
        })
        .await
        .expect("hotfolder");
    let stream = harness
        .database
        .create_stream_channel(rd_db::NewStreamChannel {
            url: "https://example.test/live".to_owned(),
            name: "Channel".to_owned(),
            quality: Some("720p".to_owned()),
            category_id: Some(category.id),
            enabled: false,
            recording: rd_core::RecordingPolicy::default(),
        })
        .await
        .expect("stream");
    let proxy_ref = harness
        .secrets
        .put_string("proxy-password".to_owned())
        .await
        .expect("proxy secret");
    let proxy = harness
        .database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "SOCKS".to_owned(),
            kind: rd_core::ProxyKind::Socks5,
            endpoint: "socks5h://127.0.0.1:1080".parse().expect("proxy URL"),
            username: Some("proxy-user".to_owned()),
            secret_ref: Some(proxy_ref.clone()),
        })
        .await
        .expect("proxy");
    let account_ref = harness
        .secrets
        .put_string("account-password".to_owned())
        .await
        .expect("account secret");
    let cookie_ref = harness
        .secrets
        .put_string("session=cookie".to_owned())
        .await
        .expect("cookies");
    let account = harness
        .database
        .create_account(rd_db::NewAccount {
            provider: "premiumize".to_owned(),
            label: "Premium".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some(account_ref.clone()),
            cookie_ref: Some(cookie_ref.clone()),
            proxy_profile_id: Some(proxy.id),
            enabled: true,
        })
        .await
        .expect("account");
    let server_ref = harness
        .secrets
        .put_string("nntp-password".to_owned())
        .await
        .expect("server secret");
    let server = harness
        .database
        .create_usenet_server(rd_db::NewUsenetServer {
            name: "News".to_owned(),
            host: "news.example.test".to_owned(),
            port: 563,
            tls: true,
            username: Some("reader".to_owned()),
            password_ref: Some(server_ref.clone()),
            proxy_profile_id: Some(proxy.id),
            priority: 5,
            max_connections: 8,
            enabled: true,
        })
        .await
        .expect("server");
    let profile_ref = harness
        .secrets
        .put_string(AUTH_PROFILE_TOKEN.to_owned())
        .await
        .expect("auth profile secret");
    let profile = harness
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "Intranet".to_owned(),
            scope: rd_core::AuthScope::parse("files.example.test", true).expect("scope"),
            method: rd_core::AuthMethod::Bearer,
            origin: rd_core::AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(profile_ref.clone()),
            certificate_ref: None,
        })
        .await
        .expect("auth profile");
    SeededConfig {
        auth_profile_id: profile.id,
        root_id: root.id,
        category_id: category.id,
        rule_id: rule.id,
        hotfolder_id: hotfolder.id,
        stream_id: stream.id,
        proxy_id: proxy.id,
        account_id: account.id,
        server_id: server.id,
        old_refs: vec![proxy_ref, account_ref, cookie_ref, server_ref, profile_ref],
    }
}

async fn export(router: &Router, include_secrets: bool) -> serde_json::Value {
    let (status, bundle) = post(
        router,
        "/api/v1/settings/export",
        serde_json::json!({
            "include_secrets": include_secrets,
            "passphrase": include_secrets.then_some("backup-passphrase")
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    bundle
}

async fn import(
    router: &Router,
    bundle: serde_json::Value,
    passphrase: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    post(
        router,
        "/api/v1/settings/import",
        serde_json::json!({ "bundle": bundle, "passphrase": passphrase }),
    )
    .await
}

#[tokio::test]
async fn full_round_trip_preserves_ids_and_rekeys_secrets() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;
    let bundle = export(&harness.router, true).await;
    let encoded = bundle.to_string();
    assert!(bundle["secrets"].is_object());
    for plaintext in [
        "proxy-password",
        "account-password",
        "session=cookie",
        "nntp-password",
    ] {
        assert!(!encoded.contains(plaintext));
    }
    harness
        .database
        .create_category(rd_db::NewCategory {
            name: "Temporary".to_owned(),
            color: "#123456".to_owned(),
            storage_root_id: seeded.root_id,
            relative_path: "temp".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("temporary category");

    let (status, summary) = import(&harness.router, bundle, Some("backup-passphrase")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["categories"], 1);
    assert_eq!(
        harness.database.list_storage_roots().await.expect("roots")[0].id,
        seeded.root_id
    );
    assert_eq!(
        harness
            .database
            .list_categories()
            .await
            .expect("categories")[0]
            .id,
        seeded.category_id
    );
    assert_eq!(
        harness.database.list_category_rules().await.expect("rules")[0].id,
        seeded.rule_id
    );
    assert_eq!(
        harness
            .database
            .list_hotfolders()
            .await
            .expect("hotfolders")[0]
            .id,
        seeded.hotfolder_id
    );
    assert_eq!(
        harness
            .database
            .list_stream_channels()
            .await
            .expect("streams")[0]
            .id,
        seeded.stream_id
    );
    assert_eq!(
        harness
            .database
            .list_proxy_profiles()
            .await
            .expect("proxies")[0]
            .id,
        seeded.proxy_id
    );
    assert_eq!(
        harness.database.list_accounts().await.expect("accounts")[0].id,
        seeded.account_id
    );
    assert_eq!(
        harness
            .database
            .list_usenet_servers()
            .await
            .expect("servers")[0]
            .id,
        seeded.server_id
    );
    let new_refs = harness
        .database
        .account_secret_refs(seeded.account_id)
        .await
        .expect("refs")
        .expect("account");
    assert_eq!(
        harness
            .secrets
            .get(new_refs.0.as_deref().expect("secret ref"))
            .await
            .expect("secret")
            .expose_secret(),
        "account-password"
    );
    assert_eq!(
        harness
            .secrets
            .get(new_refs.1.as_deref().expect("cookie ref"))
            .await
            .expect("cookies")
            .expose_secret(),
        "session=cookie"
    );
    for reference in seeded.old_refs {
        assert!(harness.secrets.get(&reference).await.is_err());
    }
}

#[tokio::test]
async fn wrong_passphrase_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;
    let bundle = export(&harness.router, true).await;
    let (status, error) = import(&harness.router, bundle, Some("wrong-passphrase")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "settings.backup_passphrase_invalid");
    assert_eq!(
        harness
            .database
            .list_categories()
            .await
            .expect("categories")[0]
            .id,
        seeded.category_id
    );
    assert!(harness.secrets.get(&seeded.old_refs[1]).await.is_ok());
}

#[tokio::test]
async fn unsupported_version_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let mut bundle = export(&harness.router, false).await;
    bundle["version"] = serde_json::json!(999);
    let (status, error) = import(&harness.router, bundle, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "settings.backup_version_unsupported");
}

#[tokio::test]
async fn dangling_category_reference_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;
    let mut bundle = export(&harness.router, false).await;
    bundle["category_rules"][0]["category_id"] = serde_json::json!(rd_core::CategoryId::new());
    let (status, error) = import(&harness.router, bundle, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "settings.backup_reference_invalid");
    assert_eq!(
        harness
            .database
            .list_categories()
            .await
            .expect("categories")[0]
            .id,
        seeded.category_id
    );
}

#[tokio::test]
async fn secretless_import_removes_credentials() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;
    let bundle = export(&harness.router, false).await;
    assert!(bundle["secrets"].is_null());
    assert!(bundle["accounts"][0]["secret_slot"].is_null());
    let (status, _) = import(&harness.router, bundle, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        harness
            .database
            .account_secret_refs(seeded.account_id)
            .await
            .expect("refs"),
        Some((None, None))
    );
    assert!(!harness.database.list_accounts().await.expect("accounts")[0].has_secret);
    for reference in seeded.old_refs {
        assert!(harness.secrets.get(&reference).await.is_err());
    }
}

#[tokio::test]
async fn factory_reset_restores_runtime_defaults_without_removing_configuration() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;
    harness
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "max_active_files": 17,
                "admin_login_disabled": true,
                "global_proxy_profile_id": seeded.proxy_id
            }),
        )
        .await
        .expect("custom settings");

    let (status, settings) = post(
        &harness.router,
        "/api/v1/settings/reset",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["max_active_files"], 3);
    assert_eq!(settings["admin_login_disabled"], false);
    assert!(settings["global_proxy_profile_id"].is_null());
    let stored = harness
        .database
        .get_setting("service.settings")
        .await
        .expect("stored settings")
        .expect("settings value");
    assert_eq!(stored["max_active_files"], 3);
    assert_eq!(
        harness
            .database
            .list_categories()
            .await
            .expect("categories")[0]
            .id,
        seeded.category_id
    );
    assert_eq!(
        harness.database.list_accounts().await.expect("accounts")[0].id,
        seeded.account_id
    );
    assert!(harness.secrets.get(&seeded.old_refs[1]).await.is_ok());
}

#[tokio::test]
async fn auth_profiles_are_backed_up_without_leaking_their_credentials() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let seeded = seed_all(&harness, directory.path()).await;

    // Without secrets the profile still travels, but carries no slot and no value.
    let plain = export(&harness.router, false).await;
    let bundled = &plain["auth_profiles"][0];
    assert_eq!(bundled["id"], seeded.auth_profile_id.to_string());
    assert_eq!(bundled["host"], "files.example.test");
    assert_eq!(bundled["include_subdomains"], true);
    assert!(bundled["secret_slot"].is_null(), "{bundled}");
    let text = plain.to_string();
    assert!(
        !text.contains(AUTH_PROFILE_TOKEN),
        "no-secret export leaked"
    );
    assert!(
        !text.contains("vault://"),
        "no-secret export leaked a reference"
    );

    // With secrets the value lives in the encrypted blob, never in the plain bundle.
    let encrypted = export(&harness.router, true).await;
    let slot = encrypted["auth_profiles"][0]["secret_slot"]
        .as_str()
        .expect("secret slot");
    let without_blob = {
        let mut copy = encrypted.clone();
        copy["secrets"] = serde_json::Value::Null;
        copy.to_string()
    };
    assert!(
        !without_blob.contains(AUTH_PROFILE_TOKEN),
        "the bundle body must only reference the slot"
    );
    assert!(!without_blob.contains("vault://"));
    assert!(!slot.is_empty());

    // A round trip restores a working profile with a freshly minted reference.
    let (status, summary) = post(
        &harness.router,
        "/api/v1/settings/import",
        serde_json::json!({ "bundle": encrypted, "passphrase": "backup-passphrase" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let restored = harness
        .database
        .list_auth_profiles()
        .await
        .expect("profiles");
    assert_eq!(restored.len(), 1);
    let reference = restored[0].secret_ref.as_deref().expect("reference");
    assert_eq!(
        harness
            .secrets
            .get(reference)
            .await
            .expect("secret")
            .expose_secret(),
        AUTH_PROFILE_TOKEN
    );
    assert!(
        harness
            .database
            .match_auth_profile(&"https://files.example.test/x".parse().expect("url"))
            .await
            .expect("match")
            .is_some(),
        "the restored profile must still apply"
    );
}
