//! The link page's routes as a browser meets them (RD-1390-02): a live listener on a free port,
//! a store that only records, a scratch configuration directory.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::http::{StatusCode, header};
use tokio::sync::mpsc;

use super::{
    LinkRefusal, LinkSecret, LinkState, Linked, Store, router,
    tests::{Answer, Stored, TOKEN, fixture, recording},
};

/// The page on a port of its own, and what it announced and stored.
struct Page {
    port: u16,
    secret: String,
    linked: mpsc::UnboundedReceiver<Linked>,
    calls: Arc<Mutex<Stored>>,
    directory: PathBuf,
}

impl Page {
    fn url(&self, secret: &str) -> String {
        format!("http://127.0.0.1:{}/link/{secret}", self.port)
    }

    fn page(&self) -> String {
        self.url(&self.secret)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn nothing_linked(&mut self) -> bool {
        self.linked.try_recv().is_err() && rd_testkit::lock(&self.calls).is_empty()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// A fresh directory of this test's own, so nothing reaches the real configuration.
fn scratch(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rd-capture-link-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    directory
}

/// Binds first, because the guard checks the port the `Host` names.
async fn serve(name: &str) -> Page {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener.local_addr().expect("the bound address").port();
    let secret = Arc::new(LinkSecret::generate());
    let (announce, linked) = mpsc::unbounded_channel();
    let calls = Arc::new(Mutex::new(Stored::new()));
    let store: Store = Arc::new(recording(&calls));
    let directory = scratch(name);
    let state = LinkState {
        secret: secret.clone(),
        port,
        linked: announce,
        directory: directory.clone(),
        store,
    };
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Page {
        port,
        secret: secret.as_str().to_owned(),
        linked,
        calls,
        directory,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build a client")
}

fn body(service: &str, allow_insecure: bool) -> serde_json::Value {
    serde_json::json!({ "service": service, "token": TOKEN, "allow_insecure": allow_insecure })
}

#[tokio::test]
async fn a_wrong_secret_is_not_found() {
    let mut page = serve("wrong-secret").await;
    let wrong = LinkSecret::generate();
    for url in [
        page.url(wrong.as_str()),
        page.url(&page.secret[..32]),
        page.url(""),
    ] {
        let answer = client().get(&url).send().await.expect("answered");
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{url}");
        assert!(answer.text().await.expect("a body").is_empty());
        let answer = client()
            .post(&url)
            .header(header::ORIGIN, page.origin())
            .json(&body("http://127.0.0.1:8710", false))
            .send()
            .await
            .expect("answered");
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{url}");
        assert!(answer.text().await.expect("a body").is_empty());
    }
    assert!(page.nothing_linked());
}

#[tokio::test]
async fn a_foreign_host_is_not_found() {
    let mut page = serve("foreign-host").await;
    for host in [
        format!("evil.example:{}", page.port),
        "127.0.0.1:9".to_owned(),
    ] {
        let answer = client()
            .get(page.page())
            .header(header::HOST, &host)
            .send()
            .await
            .expect("answered");
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{host}");
        let answer = client()
            .post(page.page())
            .header(header::HOST, &host)
            .header(header::ORIGIN, page.origin())
            .json(&body("http://127.0.0.1:8710", false))
            .send()
            .await
            .expect("answered");
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{host}");
    }
    assert!(page.nothing_linked());
}

#[tokio::test]
async fn a_post_from_another_origin_is_forbidden() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let mut page = serve("foreign-origin").await;
    for origin in [Some("https://evil.example"), Some("null"), None] {
        let mut request = client()
            .post(page.page())
            .json(&body(&service.service(), false));
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let answer = request.send().await.expect("answered");
        assert_eq!(answer.status(), StatusCode::FORBIDDEN, "{origin:?}");
        assert!(
            answer
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }
    assert!(page.nothing_linked());
    assert_eq!(
        service.settings.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the token went nowhere"
    );
}

#[tokio::test]
async fn a_get_never_links() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let mut page = serve("get").await;
    let answer = client()
        .get(format!(
            "{}?service={}&token={TOKEN}&allow_insecure=true",
            page.page(),
            service.service()
        ))
        .send()
        .await
        .expect("answered");
    assert_eq!(answer.status(), StatusCode::OK);
    let html = answer.text().await.expect("the page");
    assert!(html.contains("<form"));
    assert!(!html.contains("__DATA__") && !html.contains("__LANG__"));
    assert!(page.nothing_linked());
}

#[tokio::test]
async fn the_page_never_carries_a_token() {
    let page = serve("no-token").await;
    let stored_token = "a-stored-token-that-must-never-leave-0123456789";
    std::fs::write(
        page.directory.join("capture.json"),
        r#"{"service":"http://192.168.1.10:8710/"}"#,
    )
    .expect("write capture.json");
    std::fs::write(page.directory.join("capture.token"), stored_token).expect("write the token");
    let html = client()
        .get(page.page())
        .send()
        .await
        .expect("answered")
        .text()
        .await
        .expect("the page");
    assert!(!html.contains(stored_token));
    // The stored address is offered, ticked as confirmed: it was paired unencrypted before.
    assert!(html.contains("http://192.168.1.10:8710/"));
    assert!(html.contains(r#""confirmed":true"#));
}

#[tokio::test]
async fn every_answer_says_no_store() {
    let page = serve("no-store").await;
    let found = client().get(page.page()).send().await.expect("answered");
    let forbidden = client()
        .post(page.page())
        .header(header::ORIGIN, "https://evil.example")
        .json(&body("http://127.0.0.1:8710", false))
        .send()
        .await
        .expect("answered");
    let missing = client()
        .get(page.url(LinkSecret::generate().as_str()))
        .send()
        .await
        .expect("answered");
    for (answer, status) in [
        (found, StatusCode::OK),
        (forbidden, StatusCode::FORBIDDEN),
        (missing, StatusCode::NOT_FOUND),
    ] {
        assert_eq!(answer.status(), status);
        let named = |name: header::HeaderName| {
            answer
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        assert_eq!(named(header::CACHE_CONTROL).as_deref(), Some("no-store"));
        assert_eq!(
            named(header::REFERRER_POLICY).as_deref(),
            Some("no-referrer")
        );
        let policy = named(header::CONTENT_SECURITY_POLICY).unwrap_or_default();
        assert!(policy.contains("default-src 'none'"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
        assert_eq!(named(header::ACCESS_CONTROL_ALLOW_ORIGIN), None);
    }
}

#[tokio::test]
async fn a_refusal_answers_with_its_code_only() {
    let mut page = serve("refusal").await;
    let answer = client()
        .post(page.page())
        .header(header::ORIGIN, page.origin())
        .json(&body("http://192.168.1.10:8710", false))
        .send()
        .await
        .expect("answered");
    assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.text().await.expect("a body"),
        r#"{"code":"link_insecure_unconfirmed"}"#
    );
    assert!(page.nothing_linked());
}

#[tokio::test]
async fn a_success_is_announced_once() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let mut page = serve("success").await;
    let answer = client()
        .post(page.page())
        .header(header::ORIGIN, page.origin())
        .json(&body(&service.service(), false))
        .send()
        .await
        .expect("answered");
    assert_eq!(answer.status(), StatusCode::OK);
    let text = answer.text().await.expect("a body");
    assert!(!text.contains(TOKEN), "the token is never echoed");
    let said: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(said["insecure"], false);
    let linked = page.linked.try_recv().expect("announced");
    assert_eq!(linked.service.as_str(), format!("{}/", service.service()));
    assert!(!linked.insecure);
    assert!(page.linked.try_recv().is_err(), "once");
    assert_eq!(rd_testkit::lock(&page.calls).len(), 1);
}

#[test]
fn every_link_key_exists_in_every_language() {
    let read = |source: &str| -> BTreeMap<String, String> {
        serde_json::from_str(source).expect("the catalogue parses")
    };
    let english = read(include_str!("../../locales/en.json"));
    let keys: Vec<&String> = english
        .keys()
        .filter(|key| key.starts_with("link_"))
        .collect();
    for code in [
        LinkRefusal::InvalidAddress,
        LinkRefusal::InsecureNotConfirmed,
        LinkRefusal::Unreachable,
        LinkRefusal::NotRdownloader,
        LinkRefusal::TokenRejected,
        LinkRefusal::MissingScope,
        LinkRefusal::NotStored,
    ]
    .map(LinkRefusal::code)
    .into_iter()
    .chain([
        "link_failed",
        "link_done",
        "link_done_insecure",
        "link_connect",
    ]) {
        assert!(english.contains_key(code), "{code} has no words");
    }
    for (lang, source) in [
        ("de", include_str!("../../locales/de.json")),
        ("es", include_str!("../../locales/es.json")),
        ("fr", include_str!("../../locales/fr.json")),
    ] {
        let catalogue = read(source);
        for key in &keys {
            assert!(catalogue.contains_key(*key), "{lang}: {key}");
        }
    }
}

/// The page hides or shows "Connect unencrypted" by the agent's own loopback rule
/// (`config::is_loopback`): IPv4 literals in 127.0.0.0/8, `[::1]` and `localhost`, never a name
/// that only starts with "127.".
#[tokio::test]
async fn the_page_asks_by_the_agents_loopback_rule() {
    let page = serve("loopback-rule").await;
    let html = client()
        .get(page.page())
        .send()
        .await
        .expect("answered")
        .text()
        .await
        .expect("the page");
    assert!(
        html.contains(r"/^127(\.\d{1,3}){3}$/.test(host)"),
        "the literal rule"
    );
    assert!(html.contains(r#"host === "[::1]""#));
    assert!(html.contains(r#"host === "localhost""#));
    assert!(
        !html.contains(r#"startsWith("127.")"#),
        "a prefix would take 127.0.0.1.nip.io"
    );
}
