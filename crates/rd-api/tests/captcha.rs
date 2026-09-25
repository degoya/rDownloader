//! Captcha REST surface: what the queue offers, what it accepts, and what it must never
//! hand back — the solver API key above all.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const SOLVER_KEY: &str = "top-secret-solver-key";

/// The bearer a paired browser extension or desktop agent presents on the capture surface.
const CAPTURE_BEARER: &str = "test-capture-bearer-token";

struct Harness {
    router: Router,
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    scheduler: rd_scheduler::SchedulerHandle,
}

async fn test_harness(directory: &std::path::Path) -> Harness {
    let database = rd_db::Database::open(directory.join("captcha.sqlite3"))
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
        scheduler.clone(),
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
        scheduler,
    }
}

impl Harness {
    async fn call(&self, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, value)
    }

    async fn get(&self, uri: &str) -> (StatusCode, serde_json::Value) {
        self.call(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.call(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
    }

    /// A request as the browser extension makes it: bearer capture token, no session.
    async fn capture(
        &self,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.call(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
    }

    /// Parks a resolver on a Turnstile challenge and returns the pending entry the web
    /// interface sees, once the queue has it.
    async fn park_turnstile(
        &self,
    ) -> (
        tokio::task::JoinHandle<Result<rd_plugin_api::CaptchaAnswer, rd_core::Failure>>,
        serde_json::Value,
    ) {
        let captcha = self.scheduler.captcha().clone();
        let resolver = tokio::spawn(async move {
            captcha
                .solve_within(
                    rd_plugin_api::CaptchaChallenge::Turnstile(rd_plugin_api::WidgetChallenge {
                        site_key: "0x4AAA".to_owned(),
                        page_url: "https://ddownload.com/login.html".to_owned(),
                        invisible: false,
                    }),
                    std::time::Duration::from_secs(300),
                )
                .await
        });
        for _ in 0..600 {
            let (_, value) = self.get("/api/v1/captchas").await;
            if let Some(first) = value.as_array().and_then(|items| items.first()) {
                return (resolver, first.clone());
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the challenge never reached the queue");
    }
}

/// The one rule the whole feature hangs on: a stored solver key leaves the process only in
/// a request to the solver service, never in an API response.
#[tokio::test]
async fn the_solver_key_is_stored_by_reference_and_never_returned() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let (status, saved) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({
                "solver": "two_captcha_compatible",
                "endpoint": "https://api.capmonster.cloud",
                "api_key": SOLVER_KEY,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["has_api_key"], true);
    assert!(
        !saved.to_string().contains(SOLVER_KEY),
        "the save response echoed the key back"
    );

    let (status, config) = harness.get("/api/v1/captcha-config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["has_api_key"], true);
    assert_eq!(config["endpoint"], "https://api.capmonster.cloud");
    assert!(
        !config.to_string().contains(SOLVER_KEY),
        "reading the configuration exposed the key"
    );

    // Stored as a reference in the settings row, with the value itself in the secret store.
    let row = harness
        .database
        .get_setting(rd_captcha::SETTINGS_KEY)
        .await
        .expect("settings")
        .expect("row");
    assert!(
        !row.to_string().contains(SOLVER_KEY),
        "the settings row holds the key in clear text"
    );
    let reference = row["api_key_ref"].as_str().expect("reference");
    assert_eq!(
        harness
            .secrets
            .get(reference)
            .await
            .map(|key| secrecy::ExposeSecret::expose_secret(&key).to_owned())
            .expect("stored key"),
        SOLVER_KEY
    );
}

#[tokio::test]
async fn clearing_the_key_removes_it_from_the_secret_store() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "api_key": SOLVER_KEY }),
        )
        .await;
    let row = harness
        .database
        .get_setting(rd_captcha::SETTINGS_KEY)
        .await
        .expect("settings")
        .expect("row");
    let reference = row["api_key_ref"].as_str().expect("reference").to_owned();

    let (status, cleared) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "clear_api_key": true }),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["has_api_key"], false);
    assert!(
        harness.secrets.get(&reference).await.is_err(),
        "the abandoned key is still readable from the secret store"
    );
}

#[tokio::test]
async fn a_malformed_configuration_is_refused_with_a_code_the_ui_can_translate() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let (status, error) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "endpoint": "http://api.2captcha.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error["code"], "captcha.endpoint_invalid",
        "a plain-HTTP endpoint would put the key on the wire"
    );

    let (status, error) = harness
        .send(
            "PUT",
            "/api/v1/captcha-config",
            serde_json::json!({ "api_key": "   " }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.api_key_invalid");
}

#[tokio::test]
async fn answering_a_captcha_nobody_is_waiting_for_reports_it_plainly() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let (status, listed) = harness.get("/api/v1/captchas").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed, serde_json::json!([]));

    let gone = rd_core::CaptchaId::new();
    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{gone}/solution"),
            serde_json::json!({ "token": "42" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "captcha.not_waiting");

    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{gone}/solution"),
            serde_json::json!({ "token": "  " }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.token_invalid");
}

/// The queue is the same one the resolver waits on, so a challenge raised behind the API
/// shows up in it — and a widget challenge refuses the answer the user might try to type.
#[tokio::test]
async fn a_waiting_widget_challenge_is_listed_but_refuses_a_typed_answer() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let captcha = harness.scheduler.captcha().clone();
    let resolver = tokio::spawn(async move {
        captcha
            .solve_within(
                rd_plugin_api::CaptchaChallenge::HCaptcha(rd_plugin_api::WidgetChallenge {
                    site_key: "site".to_owned(),
                    page_url: "https://katfile.biz/file".to_owned(),
                    invisible: false,
                }),
                std::time::Duration::from_secs(300),
            )
            .await
    });

    let mut listed = serde_json::Value::Null;
    for _ in 0..600 {
        let (_, value) = harness.get("/api/v1/captchas").await;
        if value.as_array().is_some_and(|items| !items.is_empty()) {
            listed = value;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let pending = &listed[0];
    assert_eq!(pending["kind"], "h_captcha");
    assert_eq!(pending["host"], "katfile.biz");
    assert!(
        pending.get("image").is_none(),
        "a widget has no picture to show"
    );
    let id = pending["id"].as_str().expect("id").to_owned();

    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/solution"),
            serde_json::json!({ "token": "guessed" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.widget_needs_solver");

    let (status, _) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/skip"),
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let failure = resolver.await.expect("resolver").expect_err("declined");
    assert_eq!(failure.code.as_deref(), Some("captcha.skipped"));
}

#[tokio::test]
async fn testing_a_solver_that_was_never_configured_says_which_part_is_missing() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let (status, error) = harness
        .send(
            "POST",
            "/api/v1/captcha-config/test",
            serde_json::Value::Object(serde_json::Map::new()),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.solver_key_missing");

    // An endpoint typed into the test field is held to the same rule as a saved one.
    let (status, error) = harness
        .send(
            "POST",
            "/api/v1/captcha-config/test",
            serde_json::json!({ "endpoint": "http://api.2captcha.com", "api_key": "k" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.endpoint_invalid");
}

/// The browser extension's path through the API, end to end (RD-108-02): the service knows
/// nothing is connected until the extension polls, the poll that names itself flips that,
/// the token it harvested reaches the resolver through the capture route, and the token is
/// in no response along the way.
#[tokio::test]
async fn the_browser_extensions_poll_is_seen_and_its_token_reaches_the_resolver_unechoed() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let (resolver, pending) = harness.park_turnstile().await;
    let id = pending["id"].as_str().expect("id").to_owned();

    let (status, answerers) = harness.get("/api/v1/captcha-answerers").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answerers["browser_extension_connected"], false);
    assert!(answerers.get("browser_extension_seen_at").is_none());

    // Without the capture token the surface is closed, whatever the session says.
    let (status, _) = harness
        .get("/api/v1/capture/captchas?client=browser_extension")
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, widgets) = harness
        .capture(
            "GET",
            "/api/v1/capture/captchas?client=browser_extension",
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(widgets[0]["id"], id);
    assert_eq!(widgets[0]["kind"], "turnstile");
    assert_eq!(widgets[0]["page_url"], "https://ddownload.com/login.html");
    assert!(
        widgets[0].get("image").is_none() && widgets[0].get("prompt").is_none(),
        "the capture view carries what a browser needs to open the page and nothing else"
    );

    let (status, answerers) = harness.get("/api/v1/captcha-answerers").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answerers["browser_extension_connected"], true);
    assert!(answerers["browser_extension_seen_at"].is_string());

    const TOKEN: &str = "0.turnstile-token-from-the-real-browser";
    let (status, answered) = harness
        .capture(
            "POST",
            &format!("/api/v1/capture/captchas/{id}/token"),
            serde_json::json!({ "token": TOKEN }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answered["code"], "captcha.solved");
    assert!(
        !answered.to_string().contains(TOKEN),
        "the answer route echoed the token back"
    );

    let solution = resolver.await.expect("resolver").expect("solved");
    assert_eq!(
        solution,
        rd_plugin_api::CaptchaAnswer::Token(TOKEN.to_owned())
    );
    let (_, listed) = harness.get("/api/v1/captchas").await;
    assert_eq!(listed, serde_json::json!([]));

    let (status, error) = harness
        .capture(
            "POST",
            &format!("/api/v1/capture/captchas/{id}/token"),
            serde_json::json!({ "token": TOKEN }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "captcha.not_waiting");
}

/// Closing the hoster tab without answering is a decline, reported through the same capture
/// route the desktop agent uses when its window is closed, and the download fails with
/// `captcha.skipped` rather than waiting out its timeout.
#[tokio::test]
async fn a_tab_closed_without_an_answer_declines_the_captcha_through_the_capture_route() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let (resolver, pending) = harness.park_turnstile().await;
    let id = pending["id"].as_str().expect("id").to_owned();

    let (status, declined) = harness
        .capture(
            "POST",
            &format!("/api/v1/capture/captchas/{id}/skip"),
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(declined["code"], "captcha.skipped");

    let failure = resolver.await.expect("resolver").expect_err("declined");
    assert_eq!(failure.code.as_deref(), Some("captcha.skipped"));
}

/// RD-120-45: the extension opened the hoster's page and found no widget on it, because the
/// browser was already signed in there. The waiting sign-in ends at once with its own code and
/// the hoster named, instead of waiting out its timeout with nothing on screen saying why.
#[tokio::test]
async fn a_page_without_its_widget_ends_the_wait_with_a_reason_through_the_capture_route() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let (resolver, pending) = harness.park_turnstile().await;
    let id = pending["id"].as_str().expect("id").to_owned();

    let (status, reported) = harness
        .capture(
            "POST",
            &format!("/api/v1/capture/captchas/{id}/no-widget"),
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reported["code"], "captcha.page_without_widget_reported");

    let failure = resolver.await.expect("resolver").expect_err("ended");
    assert_eq!(failure.code.as_deref(), Some("captcha.page_without_widget"));
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("ddownload.com")
    );
    let (_, listed) = harness.get("/api/v1/captchas").await;
    assert_eq!(listed, serde_json::json!([]), "it stops being offered");

    let (status, error) = harness
        .capture(
            "POST",
            &format!("/api/v1/capture/captchas/{id}/no-widget"),
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "captcha.not_waiting");
}

/// RD-110-15: a click-point captcha reaches the interface as a picture, refuses text, and
/// the spot the person clicked reaches the waiting resolver as a coordinate.
#[tokio::test]
async fn a_click_point_captcha_is_answered_by_a_click_and_refuses_text() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let captcha = harness.scheduler.captcha().clone();
    let resolver = tokio::spawn(async move {
        captcha
            .solve_within(
                rd_plugin_api::CaptchaChallenge::ClickPoint(rd_plugin_api::ImageChallenge {
                    mime: "image/png".to_owned(),
                    data: b"BM".to_vec(),
                    prompt: Some("Click the circle".to_owned()),
                }),
                std::time::Duration::from_secs(300),
            )
            .await
    });

    let mut listed = serde_json::Value::Null;
    for _ in 0..600 {
        let (_, value) = harness.get("/api/v1/captchas").await;
        if value.as_array().is_some_and(|items| !items.is_empty()) {
            listed = value;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let pending = &listed[0];
    assert_eq!(pending["kind"], "click_point");
    assert_eq!(pending["image"], "data:image/png;base64,Qk0=");
    assert_eq!(pending["prompt"], "Click the circle");
    let id = pending["id"].as_str().expect("id").to_owned();

    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/solution"),
            serde_json::json!({ "token": "typed" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.answer_shape");

    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/click"),
            serde_json::json!({ "x": 99_999, "y": 3 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.point_invalid");

    let (status, answered) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/click"),
            serde_json::json!({ "x": 120, "y": 44 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answered["code"], "captcha.solved");

    let answer = resolver.await.expect("resolver").expect("solved");
    assert_eq!(
        answer,
        rd_plugin_api::CaptchaAnswer::Point(rd_plugin_api::ClickPoint { x: 120, y: 44 })
    );
    let (_, listed) = harness.get("/api/v1/captchas").await;
    assert_eq!(listed, serde_json::json!([]));
}

/// A click cannot answer an image captcha; the challenge keeps waiting for text.
#[tokio::test]
async fn a_click_is_refused_for_an_image_captcha_which_keeps_waiting() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let captcha = harness.scheduler.captcha().clone();
    let resolver = tokio::spawn(async move {
        captcha
            .solve_within(
                rd_plugin_api::CaptchaChallenge::Image(rd_plugin_api::ImageChallenge {
                    mime: "image/png".to_owned(),
                    data: b"BM".to_vec(),
                    prompt: None,
                }),
                std::time::Duration::from_secs(300),
            )
            .await
    });
    let mut id = String::new();
    for _ in 0..600 {
        let (_, value) = harness.get("/api/v1/captchas").await;
        if let Some(first) = value.as_array().and_then(|items| items.first()) {
            id = first["id"].as_str().expect("id").to_owned();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let (status, error) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/click"),
            serde_json::json!({ "x": 1, "y": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "captcha.answer_shape");

    let (status, _) = harness
        .send(
            "POST",
            &format!("/api/v1/captchas/{id}/solution"),
            serde_json::json!({ "token": "42" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "the challenge was still waiting");
    assert_eq!(
        resolver.await.expect("resolver").expect("solved"),
        rd_plugin_api::CaptchaAnswer::Token("42".to_owned())
    );
}

/// RD-110-15: a CutCaptcha with no solver configured is refused at once, with a code that
/// names the fix — never queued, never offered to a browser, and never an empty token.
#[tokio::test]
async fn a_cutcaptcha_without_a_solver_is_refused_with_a_code_and_never_queued() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;

    let started = std::time::Instant::now();
    let failure = harness
        .scheduler
        .captcha()
        .solve_within(
            rd_plugin_api::CaptchaChallenge::Cutcaptcha(rd_plugin_api::CutcaptchaChallenge {
                site_key: "SAs61IAI".to_owned(),
                misery_key: "a1488b66da00bf332a1488993a5443c79047e752".to_owned(),
                page_url: "https://filecrypt.cc/Container/ABC.html".to_owned(),
            }),
            std::time::Duration::from_secs(300),
        )
        .await
        .expect_err("nobody can answer a CutCaptcha without a solver");
    assert_eq!(
        failure.code.as_deref(),
        Some("captcha.cutcaptcha_needs_solver")
    );
    assert_eq!(failure.category, rd_core::FailureKind::NeedsCaptcha);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the refusal must not wait out the manual timeout"
    );

    let (_, listed) = harness.get("/api/v1/captchas").await;
    assert_eq!(listed, serde_json::json!([]), "never queued for a person");
    let (_, widgets) = harness
        .capture(
            "GET",
            "/api/v1/capture/captchas?client=browser_extension",
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(widgets, serde_json::json!([]), "never offered to a browser");
}
