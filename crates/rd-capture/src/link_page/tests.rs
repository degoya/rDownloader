//! The link page's guards and its verification (RD-1390-02); the routes are `route_tests.rs`.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use url::Url;

use super::{
    LinkRefusal, LinkRequest, LinkSecret, Linked, host_allowed, origin_allowed, secret_matches,
    verify_and_save,
};

/// A token of the length the service hands out, as far as the agent checks it.
pub(crate) const TOKEN: &str = "0123456789abcdef0123456789abcdef0123";

/// What a store was handed: address, token, confirmation.
pub(crate) type Stored = Vec<(Url, String, bool)>;

/// A store that records what it is handed and keeps nothing.
pub(crate) fn recording(
    calls: &Arc<Mutex<Stored>>,
) -> impl Fn(&Url, &str, bool) -> anyhow::Result<()> + Send + Sync + 'static {
    let calls = calls.clone();
    move |service: &Url, token: &str, insecure: bool| {
        rd_testkit::lock(&calls).push((service.clone(), token.to_owned(), insecure));
        Ok(())
    }
}

/// How the fixture service answers the agent-settings request.
#[derive(Clone, Copy)]
pub(crate) enum Answer {
    /// The settings, for [`TOKEN`] only; any other token is a 401.
    Accept,
    Refuse(StatusCode),
}

/// A service on a free local port: its health route names `name`, its settings route answers
/// as `answer` says. Counts every request and, apart, the settings requests that carry a token.
pub(crate) struct Fixture {
    pub(crate) server: rd_testkit::TestServer,
    pub(crate) requests: Arc<AtomicUsize>,
    pub(crate) settings: Arc<AtomicUsize>,
}

impl Fixture {
    pub(crate) fn service(&self) -> String {
        self.server.url("")
    }
}

pub(crate) async fn fixture(name: &'static str, answer: Answer) -> Fixture {
    let requests = Arc::new(AtomicUsize::new(0));
    let settings = Arc::new(AtomicUsize::new(0));
    let health_count = requests.clone();
    let (settings_count, settings_seen) = (requests.clone(), settings.clone());
    let router = Router::new()
        .route(
            "/api/v1/health",
            get(move || {
                health_count.fetch_add(1, Ordering::SeqCst);
                async move { Json(serde_json::json!({ "service": name, "version": "1.39.0" })) }
            }),
        )
        .route(
            "/api/v1/capture/agent-settings",
            get(move |headers: HeaderMap| {
                settings_count.fetch_add(1, Ordering::SeqCst);
                settings_seen.fetch_add(1, Ordering::SeqCst);
                async move {
                    let expected = format!("Bearer {TOKEN}");
                    let bearer = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok());
                    match answer {
                        Answer::Refuse(status) => status.into_response(),
                        Answer::Accept if bearer == Some(expected.as_str()) => {
                            Json(rd_core::CaptureAgentSettings::default()).into_response()
                        }
                        Answer::Accept => StatusCode::UNAUTHORIZED.into_response(),
                    }
                }
            }),
        );
    Fixture {
        server: rd_testkit::serve(router).await,
        requests,
        settings,
    }
}

pub(crate) fn request(service: &str, token: &str, allow_insecure: bool) -> LinkRequest {
    LinkRequest {
        service: service.to_owned(),
        token: token.to_owned(),
        allow_insecure,
    }
}

/// A local port nothing listens on any more.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    listener.local_addr().expect("the bound address").port()
}

#[test]
fn a_secret_is_64_hex_chars_and_never_twice() {
    let first = LinkSecret::generate();
    let second = LinkSecret::generate();
    assert_eq!(first.as_str().len(), 64);
    assert!(first.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(first.as_str(), second.as_str());
}

#[test]
fn debug_never_shows_the_secret() {
    let secret = LinkSecret::generate();
    assert!(!format!("{secret:?}").contains(secret.as_str()));
}

#[test]
fn only_loopback_hosts_on_our_port_pass() {
    let host = |value: &'static str| host_allowed(Some(&HeaderValue::from_static(value)), 9666);
    assert!(host("127.0.0.1:9666"));
    assert!(host("[::1]:9666"));
    assert!(host("LOCALHOST:9666"));
    assert!(!host("127.0.0.1:9667"), "another port");
    assert!(!host("127.0.0.1"), "no port");
    assert!(!host("evil.example:9666"), "DNS rebinding");
    assert!(!host("127.0.0.1.evil.example:9666"));
    assert!(!host_allowed(None, 9666));
}

#[test]
fn a_post_needs_our_origin() {
    let origin = |value: &'static str| origin_allowed(Some(&HeaderValue::from_static(value)), 9666);
    assert!(origin("http://127.0.0.1:9666"));
    assert!(origin("http://localhost:9666"));
    assert!(origin("http://[::1]:9666"));
    assert!(!origin("null"), "a sandboxed frame or a file");
    assert!(!origin("https://evil.example"));
    assert!(!origin("http://evil.example:9666"));
    assert!(
        !origin("https://127.0.0.1:9666"),
        "not this listener's scheme"
    );
    assert!(!origin("http://127.0.0.1:8710"), "another port");
    assert!(!origin_allowed(None, 9666));
}

#[test]
fn a_wrong_secret_never_matches() {
    let secret = LinkSecret::generate();
    assert!(secret_matches(secret.as_str(), &secret));
    assert!(!secret_matches(LinkSecret::generate().as_str(), &secret));
    assert!(!secret_matches(&secret.as_str()[..63], &secret), "a prefix");
    assert!(!secret_matches(&format!("{}0", secret.as_str()), &secret));
    assert!(!secret_matches("", &secret));
}

#[tokio::test]
async fn no_scheme_is_an_invalid_address() {
    let calls = Arc::new(Mutex::new(Stored::new()));
    for service in [
        "192.168.1.10:8710",
        "localhost:8710",
        "",
        "ftp://nas.example/",
    ] {
        let result = verify_and_save(request(service, TOKEN, true), recording(&calls)).await;
        assert_eq!(result, Err(LinkRefusal::InvalidAddress), "{service:?}");
    }
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn remote_http_needs_the_confirmation() {
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result = verify_and_save(
        request("http://192.168.1.10:8710", TOKEN, false),
        recording(&calls),
    )
    .await;
    assert_eq!(result, Err(LinkRefusal::InsecureNotConfirmed));
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn a_padded_token_is_trimmed() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let padded = format!("  {TOKEN}\r\n");
    let result = verify_and_save(
        request(&format!(" {} ", service.service()), &padded, false),
        recording(&calls),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(rd_testkit::lock(&calls)[0].1, TOKEN);
}

#[tokio::test]
async fn a_short_token_is_rejected_without_a_request() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result = verify_and_save(
        request(&service.service(), &TOKEN[..31], false),
        recording(&calls),
    )
    .await;
    assert_eq!(result, Err(LinkRefusal::TokenRejected));
    assert_eq!(service.requests.load(Ordering::SeqCst), 0);
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn a_trailing_slash_is_harmless() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let with_slash = format!("{}/", service.service());
    let result = verify_and_save(request(&with_slash, TOKEN, false), recording(&calls)).await;
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn a_refused_token_is_not_stored() {
    let service = fixture("rDownloader", Answer::Refuse(StatusCode::UNAUTHORIZED)).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result =
        verify_and_save(request(&service.service(), TOKEN, false), recording(&calls)).await;
    assert_eq!(result, Err(LinkRefusal::TokenRejected));
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn a_token_without_the_scope_is_named() {
    let service = fixture("rDownloader", Answer::Refuse(StatusCode::FORBIDDEN)).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result =
        verify_and_save(request(&service.service(), TOKEN, false), recording(&calls)).await;
    assert_eq!(result, Err(LinkRefusal::MissingScope));
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn a_foreign_listener_is_not_rdownloader() {
    let service = fixture("some other program", Answer::Accept).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result =
        verify_and_save(request(&service.service(), TOKEN, false), recording(&calls)).await;
    assert_eq!(result, Err(LinkRefusal::NotRdownloader));
    assert_eq!(
        service.settings.load(Ordering::SeqCst),
        0,
        "the token never went to it"
    );
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let port = closed_port().await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result = verify_and_save(
        request(&format!("http://127.0.0.1:{port}"), TOKEN, false),
        recording(&calls),
    )
    .await;
    assert_eq!(result, Err(LinkRefusal::Unreachable));
    assert!(rd_testkit::lock(&calls).is_empty());
}

#[tokio::test]
async fn a_good_token_is_stored_once() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result =
        verify_and_save(request(&service.service(), TOKEN, false), recording(&calls)).await;
    let address = Url::parse(&service.service()).expect("the fixture's address");
    assert_eq!(
        result,
        Ok(Linked {
            service: address.clone(),
            insecure: false
        })
    );
    let calls = rd_testkit::lock(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (address, TOKEN.to_owned(), false));
}

#[tokio::test]
async fn a_store_that_fails_is_named() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let result = verify_and_save(
        request(&service.service(), TOKEN, false),
        |_: &Url, _: &str, _: bool| Err(anyhow::anyhow!("no keyring and no fallback")),
    )
    .await;
    assert_eq!(result, Err(LinkRefusal::NotStored));
}

/// A name that only starts like a loopback address is another machine: the agent asks for the
/// confirmation, and so does the page, by the same rule (`route_tests`).
#[tokio::test]
async fn a_name_that_starts_like_loopback_needs_the_confirmation() {
    let calls = Arc::new(Mutex::new(Stored::new()));
    let result = verify_and_save(
        request("http://127.0.0.1.nip.io:8710", TOKEN, false),
        recording(&calls),
    )
    .await;
    assert_eq!(result, Err(LinkRefusal::InsecureNotConfirmed));
    assert!(rd_testkit::lock(&calls).is_empty());
}
