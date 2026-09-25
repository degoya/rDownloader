//! The two captcha kinds of RD-110-15, end to end through the broker: a click-point captcha
//! is a picture a person answers with a click, and a CutCaptcha is a solver-service matter
//! that ends in a coded refusal when there is none.

use std::time::Duration;

use axum::{Json, Router, routing::post};
use rd_captcha::{CaptchaBroker, CaptchaKind, SubmitOutcome};
use rd_core::FailureKind;
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClickPoint, CutcaptchaChallenge, ImageChallenge,
};
use serde_json::{Value, json};

/// Long enough that nothing under test can expire by accident.
const AMPLE: Duration = Duration::from_secs(300);

struct Fixture {
    broker: CaptchaBroker,
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    _directory: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = rd_db::Database::open(directory.path().join("captcha.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secrets");
    Fixture {
        broker: CaptchaBroker::new(database.clone(), secrets.clone()),
        database,
        secrets,
        _directory: directory,
    }
}

fn click_point() -> CaptchaChallenge {
    CaptchaChallenge::ClickPoint(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"BM".to_vec(),
        prompt: Some("Click the circle".to_owned()),
    })
}

fn cutcaptcha() -> CaptchaChallenge {
    CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
        site_key: "SAs61IAI".to_owned(),
        misery_key: "a1488b66da00bf332a1488993a5443c79047e752".to_owned(),
        page_url: "https://filecrypt.cc/Container/ABC.html".to_owned(),
    })
}

async fn offered(broker: &CaptchaBroker) -> rd_captcha::PendingCaptcha {
    for _ in 0..600 {
        if let Some(pending) = broker.pending().into_iter().next() {
            return pending;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no captcha was offered to the user");
}

/// The click a person makes in the web interface reaches the resolver as a coordinate; text
/// does not answer it, and the challenge waits for the click rather than failing on the text.
#[tokio::test]
async fn a_click_point_captcha_shows_its_picture_and_is_answered_by_a_click() {
    let fixture = fixture().await;
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(click_point(), AMPLE).await });

    let pending = offered(&fixture.broker).await;
    assert_eq!(pending.kind, CaptchaKind::ClickPoint);
    assert_eq!(pending.image.as_deref(), Some("data:image/png;base64,Qk0="));
    assert_eq!(pending.prompt.as_deref(), Some("Click the circle"));
    assert!(
        fixture.broker.pending_widgets().is_empty(),
        "a picture is not a browser's task"
    );
    assert_eq!(
        fixture.broker.submit(pending.id, "typed".to_owned()),
        SubmitOutcome::WrongAnswerShape
    );
    assert_eq!(
        fixture.broker.pending().len(),
        1,
        "still waiting for the click"
    );
    assert_eq!(
        fixture
            .broker
            .submit_click(pending.id, ClickPoint { x: 120, y: 44 }),
        SubmitOutcome::Delivered
    );

    let answer = resolver.await.expect("resolver").expect("solved");
    assert_eq!(answer, CaptchaAnswer::Point(ClickPoint { x: 120, y: 44 }));
    assert!(fixture.broker.pending().is_empty());
}

/// Nobody but a solver service can answer a CutCaptcha, so with none configured the answer
/// is a refusal that names the fix — at once, not after the manual timeout, and never a
/// token that happens to be empty.
#[tokio::test]
async fn a_cutcaptcha_without_a_solver_is_refused_at_once_with_a_code() {
    let fixture = fixture().await;
    let started = std::time::Instant::now();

    let failure = fixture
        .broker
        .solve_within(cutcaptcha(), AMPLE)
        .await
        .expect_err("no service, no answer");

    assert_eq!(
        failure.code.as_deref(),
        Some("captcha.cutcaptcha_needs_solver")
    );
    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a refusal must not sit out the manual timeout"
    );
    assert!(
        fixture.broker.pending().is_empty(),
        "never queued for a person"
    );
    assert!(
        fixture.broker.pending_widgets().is_empty(),
        "never offered to a browser"
    );
}

/// A configured service answers both kinds in their own shape: a coordinate task returns
/// the point, a CutCaptcha task the token, and the task carries both keys the service needs.
#[tokio::test]
async fn a_configured_solver_answers_a_click_point_and_a_cutcaptcha() {
    let fixture = fixture().await;
    let tasks: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = std::sync::Arc::default();
    let seen = tasks.clone();
    let router = Router::new()
        .route(
            "/createTask",
            post(async move |Json(body): Json<Value>| {
                let task = body["task"].clone();
                let id = if task["type"] == "CoordinatesTask" {
                    1
                } else {
                    2
                };
                seen.lock().expect("task log").push(task);
                Json(json!({ "errorId": 0, "taskId": id }))
            }),
        )
        .route(
            "/getTaskResult",
            post(async |Json(body): Json<Value>| {
                // The broker echoes the id back as a string, as the real API accepts.
                let solution = if body["taskId"] == "1" {
                    json!({ "coordinates": [{ "x": 179, "y": 154 }] })
                } else {
                    json!({ "token": "cut-token" })
                };
                Json(json!({ "errorId": 0, "status": "ready", "solution": solution }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let reference = fixture
        .secrets
        .put_string("service-key".to_owned())
        .await
        .expect("key stored");
    fixture
        .database
        .set_setting(
            rd_captcha::SETTINGS_KEY.to_owned(),
            json!({
                "solver": "two_captcha_compatible",
                "endpoint": endpoint,
                "api_key_ref": reference,
            }),
        )
        .await
        .expect("settings stored");

    let point = fixture
        .broker
        .solve_within(click_point(), AMPLE)
        .await
        .expect("service clicked");
    assert_eq!(point, CaptchaAnswer::Point(ClickPoint { x: 179, y: 154 }));

    let token = fixture
        .broker
        .solve_within(cutcaptcha(), AMPLE)
        .await
        .expect("service solved it");
    assert_eq!(token, CaptchaAnswer::Token("cut-token".to_owned()));
    assert!(
        fixture.broker.pending().is_empty(),
        "neither was ever a question for the user"
    );

    let tasks = tasks.lock().expect("task log").clone();
    assert_eq!(tasks[0]["type"], "CoordinatesTask");
    assert_eq!(tasks[0]["comment"], "Click the circle");
    assert_eq!(tasks[1]["type"], "CutCaptchaTaskProxyless");
    assert_eq!(tasks[1]["apiKey"], "SAs61IAI");
    assert_eq!(
        tasks[1]["miseryKey"],
        "a1488b66da00bf332a1488993a5443c79047e752"
    );
}
