//! End-to-end captcha solving: a resolver asks for a challenge and gets an answer back from
//! a person or from a solver service, over the real settings, secret store and event bus.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{Json, Router, extract::State, http::Uri, routing::post};
use rd_captcha::{CaptchaBroker, CaptchaKind, SubmitOutcome};
use rd_core::FailureKind;
use rd_plugin_api::{CaptchaAnswer, CaptchaChallenge, ImageChallenge, WidgetChallenge};
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

impl Fixture {
    /// Writes the settings row directly, which is also the only way to point the broker at a
    /// local solver: the REST layer accepts https endpoints only.
    async fn configure(&self, settings: Value) {
        self.database
            .set_setting(rd_captcha::SETTINGS_KEY.to_owned(), settings)
            .await
            .expect("settings stored");
    }

    async fn store_key(&self, key: &str) -> String {
        self.secrets
            .put_string(key.to_owned())
            .await
            .expect("key stored")
    }
}

fn image() -> CaptchaChallenge {
    CaptchaChallenge::Image(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"BM".to_vec(),
        prompt: Some("Type the code".to_owned()),
    })
}

fn widget() -> CaptchaChallenge {
    CaptchaChallenge::RecaptchaV2(WidgetChallenge {
        site_key: "6Lc-site-key".to_owned(),
        page_url: "https://ddownload.com/abc/file.rar".to_owned(),
        invisible: false,
    })
}

/// Waits until the resolver's challenge reaches the queue, without leaning on a timer that
/// would race the challenge's own deadline.
async fn offered(broker: &CaptchaBroker) -> rd_captcha::PendingCaptcha {
    for _ in 0..600 {
        if let Some(pending) = broker.pending().into_iter().next() {
            return pending;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no captcha was offered to the user");
}

#[tokio::test]
async fn a_typed_answer_reaches_the_resolver_that_asked_for_it() {
    let fixture = fixture().await;
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(image(), AMPLE).await });

    let pending = offered(&fixture.broker).await;
    assert_eq!(pending.kind, CaptchaKind::Image);
    assert_eq!(
        pending.image.as_deref(),
        Some("data:image/png;base64,Qk0="),
        "the picture travels inline so the UI needs no second request"
    );
    assert_eq!(
        fixture.broker.submit(pending.id, "42".to_owned()),
        SubmitOutcome::Delivered
    );

    let solution = resolver.await.expect("resolver").expect("solved");
    assert_eq!(solution, CaptchaAnswer::Token("42".to_owned()));
    assert!(fixture.broker.pending().is_empty());
}

#[tokio::test]
async fn declining_a_captcha_fails_the_download_with_a_reason_it_can_show() {
    let fixture = fixture().await;
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(image(), AMPLE).await });

    let pending = offered(&fixture.broker).await;
    assert!(fixture.broker.skip(pending.id));

    let failure = resolver.await.expect("resolver").expect_err("declined");
    assert_eq!(failure.code.as_deref(), Some("captcha.skipped"));
    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert!(fixture.broker.pending().is_empty());
}

#[tokio::test]
async fn an_unanswered_image_captcha_times_out() {
    let fixture = fixture().await;

    let failure = fixture
        .broker
        .solve_within(image(), Duration::from_millis(200))
        .await
        .expect_err("nobody answered");

    assert_eq!(failure.code.as_deref(), Some("captcha.timeout"));
    assert!(
        fixture.broker.pending().is_empty(),
        "a captcha nobody answered stops being offered"
    );
}

/// Pausing or cancelling a download drops the resolver mid-question. The captcha must go
/// with it, or the user is left staring at a prompt no download is waiting for.
#[tokio::test]
async fn a_cancelled_download_takes_its_captcha_off_the_queue() {
    let fixture = fixture().await;
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(image(), AMPLE).await });

    let pending = offered(&fixture.broker).await;
    resolver.abort();
    let _ = resolver.await;

    assert!(
        fixture.broker.pending().is_empty(),
        "the abandoned captcha {} is still being offered",
        pending.id
    );
}

/// A widget captcha is bound to the hoster's domain, so it is shown to explain the stall and
/// point at the solver settings — never to be typed into.
#[tokio::test]
async fn a_widget_challenge_is_offered_as_a_hint_that_refuses_typed_answers() {
    let fixture = fixture().await;
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(widget(), AMPLE).await });

    let pending = offered(&fixture.broker).await;
    assert_eq!(pending.kind, CaptchaKind::RecaptchaV2);
    assert_eq!(pending.host.as_deref(), Some("ddownload.com"));
    assert!(
        pending.image.is_none(),
        "there is nothing for the user to look at, let alone solve"
    );

    assert_eq!(
        fixture.broker.submit(pending.id, "guess".to_owned()),
        SubmitOutcome::WidgetNeedsSolver
    );
    assert_eq!(
        fixture.broker.pending().len(),
        1,
        "configuring a solver is still a way out, so the hint stays"
    );

    assert!(fixture.broker.skip(pending.id));
    let failure = resolver.await.expect("resolver").expect_err("declined");
    assert_eq!(failure.code.as_deref(), Some("captcha.skipped"));
}

#[tokio::test]
async fn an_ignored_widget_hint_reports_that_a_solver_is_needed() {
    let fixture = fixture().await;

    let failure = fixture
        .broker
        .solve_within(widget(), Duration::from_millis(200))
        .await
        .expect_err("nobody can answer this");

    assert_eq!(
        failure.code.as_deref(),
        Some("captcha.widget_needs_solver"),
        "the download must name the fix, not just report a timeout"
    );
}

/// The browser extension's whole path through the broker (RD-108-02): its poll is what
/// makes the service say an extension is connected, the token it harvests from the hoster's
/// page answers the widget the typed path refuses, and a tab closed without an answer
/// declines the captcha exactly as the web interface would.
#[tokio::test]
async fn a_browser_extension_is_seen_when_it_polls_and_its_token_answers_the_widget() {
    let fixture = fixture().await;
    assert!(
        !fixture.broker.answerers().browser_extension_connected,
        "nothing has polled yet, so nothing is connected"
    );

    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(widget(), AMPLE).await });
    let pending = offered(&fixture.broker).await;

    // The extension lists the widgets with its capture token and names itself doing so.
    fixture.broker.note_browser_extension();
    let widgets = fixture.broker.pending_widgets();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].page_url, "https://ddownload.com/abc/file.rar");
    let answerers = fixture.broker.answerers();
    assert!(answerers.browser_extension_connected);
    assert!(answerers.browser_extension_seen_at.is_some());

    assert_eq!(
        fixture
            .broker
            .submit_from_browser(pending.id, "0.turnstile-token".to_owned()),
        SubmitOutcome::Delivered
    );
    let solution = resolver.await.expect("resolver").expect("solved");
    assert_eq!(
        solution,
        CaptchaAnswer::Token("0.turnstile-token".to_owned())
    );
    assert!(fixture.broker.pending().is_empty());

    // Closing the tab without answering is a decline, not a timeout.
    let broker = fixture.broker.clone();
    let resolver = tokio::spawn(async move { broker.solve_within(widget(), AMPLE).await });
    let pending = offered(&fixture.broker).await;
    assert!(fixture.broker.skip(pending.id));
    let failure = resolver.await.expect("resolver").expect_err("declined");
    assert_eq!(failure.code.as_deref(), Some("captcha.skipped"));
}

#[tokio::test]
async fn with_manual_solving_off_a_challenge_is_refused_immediately() {
    let fixture = fixture().await;
    fixture
        .configure(json!({ "manual_enabled": false, "manual_timeout_seconds": 180 }))
        .await;

    let failure = fixture
        .broker
        .solve_within(image(), AMPLE)
        .await
        .expect_err("nobody to ask");

    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert!(fixture.broker.pending().is_empty());
}

/// The reservation the plugin host makes must cover what answering can actually take, or a
/// generous manual timeout would be cut off mid-answer.
#[tokio::test]
async fn the_reserved_time_follows_the_configured_timeout() {
    let fixture = fixture().await;

    fixture
        .configure(json!({ "manual_enabled": true, "manual_timeout_seconds": 600 }))
        .await;
    assert!(
        fixture.broker.solve_allowance().await >= Duration::from_secs(600),
        "a ten-minute manual timeout needs at least ten minutes reserved"
    );

    fixture
        .configure(json!({ "manual_enabled": false, "manual_timeout_seconds": 600 }))
        .await;
    let without_manual = fixture.broker.solve_allowance().await;
    assert!(
        (Duration::from_secs(30)..Duration::from_secs(120)).contains(&without_manual),
        "with nobody to ask, only a short reservation is warranted: {without_manual:?}"
    );
}

// ---------------------------------------------------------------------------
// Solver service
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct SolverCalls {
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl SolverCalls {
    fn record(&self, uri: &Uri, body: &Value) {
        self.seen
            .lock()
            .expect("call log")
            .push((uri.to_string(), body.clone()));
    }

    fn requests(&self) -> Vec<(String, Value)> {
        self.seen.lock().expect("call log").clone()
    }
}

/// Starts a stand-in for any 2captcha-compatible service and returns its base URL.
async fn solver_service(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{address}")
}

fn balance_service(calls: SolverCalls, response: Value) -> Router {
    Router::new()
        .route(
            "/getBalance",
            post(
                async move |State(calls): State<SolverCalls>, uri: Uri, Json(body): Json<Value>| {
                    calls.record(&uri, &body);
                    Json(response.clone())
                },
            ),
        )
        .with_state(calls)
}

#[tokio::test]
async fn testing_the_solver_reports_the_balance_without_ever_sending_the_key_in_a_url() {
    let fixture = fixture().await;
    let calls = SolverCalls::default();
    let endpoint = solver_service(balance_service(
        calls.clone(),
        json!({ "errorId": 0, "balance": 12.5 }),
    ))
    .await;
    let reference = fixture.store_key("secret-key").await;
    fixture
        .configure(json!({
            "solver": "two_captcha_compatible",
            "endpoint": endpoint,
            "api_key_ref": reference,
        }))
        .await;

    let balance = fixture
        .broker
        .test_solver(None, None)
        .await
        .expect("service answered");

    assert!((balance - 12.5).abs() < f64::EPSILON);
    let requests = calls.requests();
    assert_eq!(requests.len(), 1);
    let (uri, body) = &requests[0];
    assert!(
        !uri.contains("secret-key"),
        "the key must never reach a URL, where proxies and logs would keep it: {uri}"
    );
    assert_eq!(
        body["clientKey"], "secret-key",
        "the stored key is the one that gets used"
    );
}

#[tokio::test]
async fn an_unsaved_key_can_be_tested_before_it_is_stored() {
    let fixture = fixture().await;
    let calls = SolverCalls::default();
    let endpoint = solver_service(balance_service(
        calls.clone(),
        json!({ "errorId": 0, "balance": 3.0 }),
    ))
    .await;

    let balance = fixture
        .broker
        .test_solver(Some(endpoint), Some("typed-key".to_owned()))
        .await
        .expect("service answered");

    assert!((balance - 3.0).abs() < f64::EPSILON);
    assert_eq!(calls.requests()[0].1["clientKey"], "typed-key");
}

#[tokio::test]
async fn a_rejected_key_is_reported_as_the_users_problem_and_a_dead_service_as_ours() {
    let fixture = fixture().await;
    let rejected = solver_service(balance_service(
        SolverCalls::default(),
        json!({ "errorId": 1, "errorCode": "ERROR_KEY_DOES_NOT_EXIST" }),
    ))
    .await;

    let failure = fixture
        .broker
        .test_solver(Some(rejected), Some("wrong".to_owned()))
        .await
        .expect_err("key rejected");
    assert_eq!(failure.code.as_deref(), Some("captcha.solver_failed"));
    assert_eq!(
        failure.category,
        FailureKind::Permanent,
        "a bad key must not be retried for every link in the queue"
    );
    assert_eq!(
        failure.params.get("reason").map(String::as_str),
        Some("ERROR_KEY_DOES_NOT_EXIST"),
        "the UI needs the service's own wording to explain the refusal"
    );

    let busy = solver_service(balance_service(
        SolverCalls::default(),
        json!({ "errorId": 1, "errorCode": "ERROR_NO_SLOT_AVAILABLE" }),
    ))
    .await;
    let failure = fixture
        .broker
        .test_solver(Some(busy), Some("fine".to_owned()))
        .await
        .expect_err("service busy");
    assert_eq!(failure.category, FailureKind::CaptchaFailed);
}

#[tokio::test]
async fn testing_without_any_key_says_so_instead_of_calling_the_service() {
    let fixture = fixture().await;

    let failure = fixture
        .broker
        .test_solver(None, None)
        .await
        .expect_err("no key configured");

    assert_eq!(failure.code.as_deref(), Some("captcha.solver_key_missing"));
}

/// The whole point of a solver service: a widget captcha nobody could answer manually is
/// resolved without the user ever seeing it.
#[tokio::test]
async fn a_configured_solver_answers_a_widget_challenge_nobody_could_type() {
    let fixture = fixture().await;
    let calls = SolverCalls::default();
    let router = Router::new()
        .route(
            "/createTask",
            post(
                async |State(calls): State<SolverCalls>, uri: Uri, Json(body): Json<Value>| {
                    calls.record(&uri, &body);
                    Json(json!({ "errorId": 0, "taskId": 4711 }))
                },
            ),
        )
        .route(
            "/getTaskResult",
            post(
                async |State(calls): State<SolverCalls>, uri: Uri, Json(body): Json<Value>| {
                    calls.record(&uri, &body);
                    Json(json!({
                        "errorId": 0,
                        "status": "ready",
                        "solution": { "gRecaptchaResponse": "solved-token" },
                    }))
                },
            ),
        )
        .with_state(calls.clone());
    let endpoint = solver_service(router).await;
    let reference = fixture.store_key("service-key").await;
    fixture
        .configure(json!({
            "solver": "two_captcha_compatible",
            "endpoint": endpoint,
            "api_key_ref": reference,
        }))
        .await;

    let solution = fixture
        .broker
        .solve_within(widget(), AMPLE)
        .await
        .expect("service solved it");

    assert_eq!(solution, CaptchaAnswer::Token("solved-token".to_owned()));
    assert!(
        fixture.broker.pending().is_empty(),
        "a solved widget was never a question for the user"
    );
    let requests = calls.requests();
    let created = &requests[0].1;
    assert_eq!(created["task"]["type"], "RecaptchaV2TaskProxyless");
    assert_eq!(created["task"]["websiteKey"], "6Lc-site-key");
    assert!(
        requests.iter().all(|(uri, _)| !uri.contains("service-key")),
        "no request may carry the key in its URL"
    );
}
