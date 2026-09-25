//! REST contract for auth profiles, with the leak checks the acceptance criteria demand:
//! no credential value and no `vault://` reference may leave through the API.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// Capture intake is token-gated; the cookie tests speak through a real capture token.
const CAPTURE_BEARER: &str = "test-capture-bearer-token";

/// Credential values seeded by the tests; none of them may ever appear in a response.
const COOKIE_VALUE: &str = "session=super-secret-session-value";
const TOKEN_VALUE: &str = "bearer-token-must-never-be-returned";
const BASIC_PASSWORD: &str = "basic-password-must-never-be-returned";

struct Harness {
    router: Router,
    database: rd_db::Database,
}

async fn test_harness(directory: &std::path::Path) -> Harness {
    let database = rd_db::Database::open(directory.join("auth-profiles.sqlite3"))
        .await
        .expect("database");
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            rd_core::CAPTURE_SCOPE.to_owned(),
            hex::encode(Sha256::digest(CAPTURE_BEARER.as_bytes())),
            vec![rd_core::CAPTURE_SCOPE.to_owned()],
        )
        .await
        .expect("capture token");
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
    }
}

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    send(router, method, path, body, false).await
}

/// Same as `request`, but authenticated with a capture token instead of a session.
async fn capture_request(
    router: &Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, "POST", path, Some(body), true).await
}

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    capture_token: bool,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if capture_token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"));
    }
    let request = match &body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn bearer_profile(name: &str, scope: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "scope": scope,
        "include_subdomains": false,
        "method": "bearer",
        "username": null,
        "secret": TOKEN_VALUE,
        "certificate_pem": null,
        "expires_at": null,
        "enabled": true,
    })
}

/// Fails if any credential value or vault reference shows up in `value`.
fn assert_no_secrets(value: &serde_json::Value, context: &str) {
    let text = value.to_string();
    for needle in [COOKIE_VALUE, TOKEN_VALUE, BASIC_PASSWORD, "vault://"] {
        assert!(!text.contains(needle), "{context} leaked {needle}: {text}");
    }
}

#[tokio::test]
async fn profiles_round_trip_without_ever_returning_a_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, created) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(bearer_profile(
            "Intranet",
            "https://files.example.com/reports",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_no_secrets(&created, "create response");
    assert_eq!(created["host"], "files.example.com");
    assert_eq!(created["path_prefix"], "/reports");
    assert_eq!(created["has_secret"], true);
    assert_eq!(created["has_client_certificate"], false);
    assert_eq!(created["origin"], "manual");
    let id = created["id"].as_str().expect("id").to_owned();

    let (status, listed) = request(&harness.router, "GET", "/api/v1/auth-profiles", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_no_secrets(&listed, "list response");

    // Rotating the token must not surface the old or the new value.
    let (status, updated) = request(
        &harness.router,
        "PUT",
        &format!("/api/v1/auth-profiles/{id}"),
        Some(serde_json::json!({
            "name": "Intranet",
            "scope": "https://files.example.com/reports",
            "include_subdomains": true,
            "method": "bearer",
            "username": null,
            "secret": TOKEN_VALUE,
            "certificate_pem": null,
            "clear_certificate": false,
            "expires_at": null,
            "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_no_secrets(&updated, "update response");
    assert_eq!(updated["include_subdomains"], true);

    let (status, disabled) = request(
        &harness.router,
        "POST",
        &format!("/api/v1/auth-profiles/{id}/disable"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disabled["enabled"], false);

    let (status, _) = request(
        &harness.router,
        "DELETE",
        &format!("/api/v1/auth-profiles/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        harness
            .database
            .list_auth_profiles()
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn invalid_scopes_and_credentials_are_refused_with_stable_codes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let cases = [
        (
            serde_json::json!({"name": "Bad", "scope": "ftp://example.com/", "include_subdomains": false,
              "method": "bearer", "username": null, "secret": TOKEN_VALUE, "certificate_pem": null,
              "expires_at": null, "enabled": true}),
            "authprofile.insecure_scheme",
        ),
        (
            serde_json::json!({"name": "Bad", "scope": "https://user:pw@example.com/", "include_subdomains": false,
              "method": "bearer", "username": null, "secret": TOKEN_VALUE, "certificate_pem": null,
              "expires_at": null, "enabled": true}),
            "authprofile.scope_invalid",
        ),
        (
            // Bearer must not carry a username; basic must.
            serde_json::json!({"name": "Bad", "scope": "example.com", "include_subdomains": false,
              "method": "bearer", "username": "someone", "secret": TOKEN_VALUE, "certificate_pem": null,
              "expires_at": null, "enabled": true}),
            "authprofile.method_mismatch",
        ),
        (
            serde_json::json!({"name": "Bad", "scope": "example.com", "include_subdomains": false,
              "method": "basic", "username": null, "secret": BASIC_PASSWORD, "certificate_pem": null,
              "expires_at": null, "enabled": true}),
            "authprofile.method_mismatch",
        ),
        (
            serde_json::json!({"name": "Bad", "scope": "example.com", "include_subdomains": false,
              "method": "bearer", "username": null, "secret": null, "certificate_pem": null,
              "expires_at": null, "enabled": true}),
            "authprofile.credentials_required",
        ),
        (
            // A passphrase-protected key cannot be decrypted anywhere in this pipeline, so
            // it has to be refused at save time rather than at the first download.
            serde_json::json!({"name": "Bad", "scope": "example.com", "include_subdomains": false,
              "method": "bearer", "username": null, "secret": TOKEN_VALUE,
              "certificate_pem": "-----BEGIN ENCRYPTED PRIVATE KEY-----\nabc\n-----END ENCRYPTED PRIVATE KEY-----\n",
              "expires_at": null, "enabled": true}),
            "authprofile.key_encrypted",
        ),
        (
            serde_json::json!({"name": "Bad", "scope": "example.com", "include_subdomains": false,
              "method": "bearer", "username": null, "secret": TOKEN_VALUE,
              "certificate_pem": "not a certificate",
              "expires_at": null, "enabled": true}),
            "authprofile.certificate_invalid",
        ),
    ];
    for (body, expected) in cases {
        let (status, response) =
            request(&harness.router, "POST", "/api/v1/auth-profiles", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["code"], expected, "{response}");
        assert_no_secrets(&response, "error response");
    }
    assert!(
        harness
            .database
            .list_auth_profiles()
            .await
            .expect("list")
            .is_empty(),
        "a rejected request must not leave a profile behind"
    );
}

#[tokio::test]
async fn two_profiles_cannot_claim_the_same_scope() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, _) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(bearer_profile("First", "example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, response) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(bearer_profile("Second", "https://example.com/")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(response["code"], "authprofile.scope_conflict");
}

#[tokio::test]
async fn captured_cookies_land_disabled_and_inherit_their_browser_expiry() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // Netscape format: the expiry column is what the browser itself gave the session.
    let cookies = format!(
        ".example.com\tTRUE\t/\tTRUE\t2000000000\tsession\t{}",
        "abc"
    );

    let (status, created) = capture_request(
        &harness.router,
        "/api/v1/capture/cookies",
        serde_json::json!({
            "name": "Example session",
            "scope": "example.com",
            "include_subdomains": true,
            "cookies": cookies,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_no_secrets(&created, "capture response");
    assert_eq!(created["origin"], "browser_capture");
    assert_eq!(
        created["enabled"], false,
        "a capture token must not mint a usable credential"
    );
    assert!(created["expires_at"].is_string(), "{created}");

    // Until a person approves it, the profile must not be applied to anything.
    assert!(
        harness
            .database
            .match_auth_profile(&"https://example.com/f".parse().expect("url"))
            .await
            .expect("match")
            .is_none()
    );

    let id = created["id"].as_str().expect("id");
    let (status, approved) = request(
        &harness.router,
        "POST",
        &format!("/api/v1/auth-profiles/{id}/enable"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(approved["enabled"], true);
    assert!(
        harness
            .database
            .match_auth_profile(&"https://example.com/f".parse().expect("url"))
            .await
            .expect("match")
            .is_some()
    );
}

#[tokio::test]
async fn cookies_outside_the_approved_domain_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, response) = capture_request(
        &harness.router,
        "/api/v1/capture/cookies",
        serde_json::json!({
            "name": null,
            "scope": "example.com",
            "include_subdomains": true,
            // A row for a completely different site must not ride along.
            "cookies": ".evil.tld\tTRUE\t/\tTRUE\t2000000000\tsession\tabc",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "authprofile.cookie_outside_scope");

    // Nor a row for the public suffix above the scope, which every `*.co.uk` would receive.
    let (status, response) = capture_request(
        &harness.router,
        "/api/v1/capture/cookies",
        serde_json::json!({
            "name": null,
            "scope": "example.co.uk",
            "include_subdomains": false,
            "cookies": ".co.uk\tTRUE\t/\tTRUE\t2000000000\tsession\tabc",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "authprofile.cookie_public_suffix");
}

/// A cookie profile as the settings form sends it, with one Netscape row for `domain`.
fn cookie_profile(name: &str, scope: &str, domain: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "scope": scope,
        "include_subdomains": true,
        "method": "cookies",
        "username": null,
        "secret": format!("{domain}\tTRUE\t/\tTRUE\t2000000000\tsession\tabc"),
        "certificate_pem": null,
        "clear_certificate": false,
        "expires_at": null,
        "enabled": true,
    })
}

/// The download refuses a cookie set outside the profile's scope (RD-120-49); saving the
/// profile refuses it first, with the same rule and a code of its own (RD-120-54).
#[tokio::test]
async fn a_cookie_profile_is_checked_against_its_scope_when_it_is_created() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let refused = [
        (
            "example.co.uk",
            ".co.uk",
            "authprofile.cookie_public_suffix",
        ),
        (
            "www.example.com",
            ".com",
            "authprofile.cookie_public_suffix",
        ),
        (
            "example.com",
            ".evil.tld",
            "authprofile.cookie_outside_scope",
        ),
        (
            "example.com",
            "example.com.evil.tld",
            "authprofile.cookie_outside_scope",
        ),
        (
            "www.example.com",
            "dl.example.com",
            "authprofile.cookie_outside_scope",
        ),
    ];
    for (scope, domain, code) in refused {
        let (status, response) = request(
            &harness.router,
            "POST",
            "/api/v1/auth-profiles",
            Some(cookie_profile("Refused", scope, domain)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{domain}: {response}");
        assert_eq!(response["code"], code, "{domain}: {response}");
    }
    let (_, response) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(cookie_profile("Refused", "example.com", ".evil.tld")),
    )
    .await;
    assert_eq!(response["params"]["host"], "example.com", "{response}");
    assert!(
        harness
            .database
            .list_auth_profiles()
            .await
            .expect("list")
            .is_empty(),
        "a refused profile was stored"
    );

    // A parent-domain row is what the browser returns for a page (RD-120-49): allowed.
    let (status, created) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(cookie_profile("Parent", "www.example.com", ".example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

#[tokio::test]
async fn a_cookie_profile_is_checked_against_its_scope_when_it_is_changed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, created) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(cookie_profile(
            "Portal",
            "www.example.com",
            "www.example.com",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let path = format!(
        "/api/v1/auth-profiles/{}",
        created["id"].as_str().expect("id")
    );
    let put = |body: serde_json::Value| request(&harness.router, "PUT", &path, Some(body));

    let (status, response) = put(cookie_profile("Portal", "www.example.com", ".com")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "authprofile.cookie_public_suffix");
    let (status, response) = put(cookie_profile("Portal", "www.example.com", ".evil.tld")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "authprofile.cookie_outside_scope");
    assert_eq!(response["params"]["host"], "www.example.com");

    // Kept cookies meet the new scope too: moving the profile to another host refuses them,
    // renaming it does not.
    let mut kept = cookie_profile("Portal", "other.org", "unused");
    kept["secret"] = serde_json::Value::Null;
    let (status, response) = put(kept.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "authprofile.cookie_outside_scope");
    assert_eq!(response["params"]["host"], "other.org");
    kept["scope"] = "www.example.com/members".into();
    kept["name"] = "Renamed".into();
    let (status, response) = put(kept).await;
    assert_eq!(status, StatusCode::OK, "{response}");

    let (status, response) = put(cookie_profile("Portal", "www.example.com", ".example.com")).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let stored = harness.database.list_auth_profiles().await.expect("list");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].scope.host, "www.example.com");
}

#[tokio::test]
async fn a_job_can_be_pointed_at_a_profile_and_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, profile) = request(
        &harness.router,
        "POST",
        "/api/v1/auth-profiles",
        Some(bearer_profile("Intranet", "example.com")),
    )
    .await;
    let profile_id = profile["id"].as_str().expect("id").to_owned();

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "package".to_owned(),
            destination: directory.path().display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = harness
        .database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: "https://example.com/file.bin".parse().expect("url"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let path = format!("/api/v1/downloads/{}/auth-profile", download.id);

    let (status, pinned) = request(
        &harness.router,
        "PUT",
        &path,
        Some(serde_json::json!({"auth_profile": {"mode": "pinned", "id": profile_id}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    assert_eq!(pinned["auth_profile"]["mode"], "pinned");
    assert_eq!(pinned["auth_profile"]["id"], profile_id);

    let (status, none) = request(
        &harness.router,
        "PUT",
        &path,
        Some(serde_json::json!({"auth_profile": {"mode": "none"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{none}");
    assert_eq!(none["auth_profile"]["mode"], "none");

    let (status, auto) = request(
        &harness.router,
        "PUT",
        &path,
        Some(serde_json::json!({"auth_profile": {"mode": "auto"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{auto}");
    assert_eq!(auto["auth_profile"]["mode"], "auto");

    // Pinning something that does not exist must be refused, not stored as a dangling id.
    let (status, missing) = request(
        &harness.router,
        "PUT",
        &path,
        Some(serde_json::json!({
            "auth_profile": {"mode": "pinned", "id": rd_core::AuthProfileId::new().to_string()}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["code"], "authprofile.not_found");
}
