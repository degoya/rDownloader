//! Tests for [`super`]: the two obligations the executor puts on a fetcher, the honesty of
//! the resolver, and which captcha kinds cross the line.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rd_core::{Failure, FailureKind};
use rd_plugin_api::{CaptchaAnswer, CaptchaChallenge};
use rd_siterules::{CaptchaRequest, CaptchaSolver, FetchFailure, FetchRequest, Fetcher, Method};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use rd_http::test_support::{PUBLIC, connect_proxy, entries, refusing_proxy};

use super::{RuleCaptcha, RuleNetwork, RuleResolver};

/// A server that answers canned responses and remembers the request lines it saw.
struct TestServer {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    fn requests(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

async fn serve(responses: Vec<String>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut index = 0usize;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap_or_default();
            let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
            if let Some(line) = text.lines().next() {
                recorder.lock().expect("seen").push(line.to_owned());
            }
            let answer = responses
                .get(index)
                .or_else(|| responses.last())
                .cloned()
                .unwrap_or_default();
            index += 1;
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    TestServer { address, seen }
}

fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

async fn test_network(directory: &Path) -> RuleNetwork {
    network_with(directory, None).await
}

/// A rule network whose global proxy is the recording one on `port`, resolving names itself
/// when `resolves_names`.
async fn test_network_behind(directory: &Path, port: u16, resolves_names: bool) -> RuleNetwork {
    network_with(directory, Some((port, resolves_names))).await
}

async fn network_with(directory: &Path, proxy: Option<(u16, bool)>) -> RuleNetwork {
    let database = rd_db::Database::open(directory.join("rules.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    let mut defaults = rd_http::NetworkDefaults::default();
    if let Some((port, resolves_names)) = proxy {
        let profile = database
            .create_proxy_profile(rd_db::NewProxyProfile {
                name: "recording proxy".to_owned(),
                kind: rd_core::ProxyKind::Http,
                endpoint: format!("http://127.0.0.1:{port}").parse().expect("proxy"),
                username: None,
                secret_ref: None,
                resolves_names,
            })
            .await
            .expect("proxy profile");
        defaults.global_proxy_profile_id = Some(profile.id);
    }
    RuleNetwork::new(
        database,
        secrets,
        Arc::new(tokio::sync::RwLock::new(defaults)),
    )
}

fn request(url: &Url, addresses: Vec<std::net::IpAddr>) -> FetchRequest {
    FetchRequest {
        url: url.clone(),
        addresses,
        method: Method::Get,
        form: BTreeMap::new(),
        json: false,
        max_bytes: 64 * 1024,
        timeout: Duration::from_secs(10),
    }
}

/// Obligation one, and the whole point of `FetchRequest::addresses`: the executor resolved
/// the host and checked what came back, so the fetcher connects to *that* and asks no
/// resolver of its own. A name that no resolver can answer, reached anyway, is the proof —
/// and the same request without the addresses shows the name really has none, so nothing but
/// the pinning can have carried the first one through.
#[tokio::test]
async fn the_fetcher_connects_to_the_addresses_it_is_handed_and_resolves_nothing() {
    let server = serve(vec![ok("pinned")]).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network(directory.path()).await;
    // `.invalid` is reserved precisely so that it never resolves (RFC 6761).
    let url: Url = format!("http://pinned.invalid:{}/page", server.address.port())
        .parse()
        .expect("url");

    let answered = network
        .fetcher()
        .fetch(request(&url, vec![server.address.ip()]))
        .await
        .expect("the pinned address answered");
    assert_eq!(answered.status, 200);
    assert_eq!(answered.body, "pinned");
    assert_eq!(server.requests().len(), 1);

    let unpinned = network.fetcher().fetch(request(&url, Vec::new())).await;
    assert!(
        matches!(
            unpinned,
            Err(FetchFailure::Unreachable(_) | FetchFailure::Other(_))
        ),
        "without the checked addresses the name cannot be reached at all: {unpinned:?}"
    );
}

/// RD-1330-01: behind the proxy the pinning holds — the proxy is handed the address the
/// executor checked, never the name, so whatever the name would answer the proxy a moment
/// later reaches nothing. `.invalid` never resolves anywhere: the request arriving proves that
/// no resolver, here or at the proxy, was asked.
#[tokio::test]
async fn behind_the_proxy_the_fetcher_hands_it_the_checked_address() {
    let server = serve(vec![ok("pinned")]).await;
    let port = server.address.port();
    let (proxy, asked) = connect_proxy(None).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network_behind(directory.path(), proxy, false).await;
    let url: Url = format!("http://pinned.invalid:{port}/page")
        .parse()
        .expect("url");

    let answered = network
        .fetcher()
        .fetch(request(&url, vec![PUBLIC]))
        .await
        .expect("the checked address answered");

    assert_eq!(answered.status, 200);
    assert_eq!(answered.body, "pinned");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
    assert_eq!(server.requests(), ["GET /page HTTP/1.1"]);
}

/// The profile's "the proxy resolves names itself": the proxy is handed the name, as before.
#[tokio::test]
async fn with_the_proxy_resolving_names_it_is_handed_the_name() {
    let (proxy, asked) = connect_proxy(None).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network_behind(directory.path(), proxy, true).await;
    let url: Url = "http://pinned.invalid:8080/page".parse().expect("url");

    let answered = network
        .fetcher()
        .fetch(request(&url, vec![PUBLIC]))
        .await
        .expect("the proxy answered");

    // The fixture opens tunnels only; an absolute-form request is answered `502`.
    assert_eq!(answered.status, 502);
    assert_eq!(
        entries(&asked),
        ["GET http://pinned.invalid:8080/page HTTP/1.1"]
    );
}

/// A tunnel the proxy refuses reaches the run by its stable code and the proxy's status.
#[tokio::test]
async fn a_refused_tunnel_is_named_by_its_code_and_status() {
    let (proxy, asked) = refusing_proxy(403).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network_behind(directory.path(), proxy, false).await;
    let url: Url = "http://pinned.invalid:8080/page".parse().expect("url");

    let failure = network.fetcher().fetch(request(&url, vec![PUBLIC])).await;

    let reason = match failure {
        Err(FetchFailure::Other(reason)) => reason,
        other => panic!("a refused tunnel is a failure with a reason: {other:?}"),
    };
    assert!(reason.starts_with(rd_http::CODE_TUNNEL_REFUSED), "{reason}");
    assert!(reason.contains("403"), "{reason}");
    assert_eq!(entries(&asked), [format!("CONNECT {PUBLIC}:8080 HTTP/1.1")]);
}

/// Obligation two: a client that followed redirects on its own would hide every hop from the
/// executor, and the hop is exactly what has to pass the host and address bolts again.
#[tokio::test]
async fn the_fetcher_reports_a_redirect_instead_of_following_it() {
    let server = serve(vec![
        "HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n".to_owned(),
        ok("followed"),
    ])
    .await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network(directory.path()).await;
    let url: Url = format!("http://redirect.invalid:{}/start", server.address.port())
        .parse()
        .expect("url");

    let answered = network
        .fetcher()
        .fetch(request(&url, vec![server.address.ip()]))
        .await
        .expect("answered");
    assert_eq!(answered.status, 302);
    assert_eq!(answered.header("Location"), Some("/elsewhere"));
    assert_eq!(
        server.requests().len(),
        1,
        "the second page was never asked for"
    );
}

/// The byte ceiling is passed to the adapter as well as checked by the executor, so a body
/// that runs away is dropped here rather than read to its end first.
#[tokio::test]
async fn a_body_past_the_ceiling_stops_being_read() {
    let server = serve(vec![ok(&"a".repeat(4096))]).await;
    let directory = tempfile::tempdir().expect("temporary directory");
    let network = test_network(directory.path()).await;
    let url: Url = format!("http://large.invalid:{}/page", server.address.port())
        .parse()
        .expect("url");
    let mut wanted = request(&url, vec![server.address.ip()]);
    wanted.max_bytes = 128;

    let refused = network.fetcher().fetch(wanted).await;
    assert!(
        matches!(refused, Err(FetchFailure::TooLarge)),
        "{refused:?}"
    );
}

/// The resolver answers and judges nothing: the ban on private and loopback addresses is the
/// executor's bolt, applied to what this returns. If the adapter filtered here instead, a
/// name resolving into this network would look like a name with no address at all.
#[tokio::test]
async fn the_resolver_reports_what_the_system_says_and_judges_nothing() {
    let addresses = rd_siterules::HostResolver::resolve(&RuleResolver, "localhost")
        .await
        .expect("localhost resolves");
    assert!(
        addresses.iter().any(std::net::IpAddr::is_loopback),
        "a loopback answer reaches the executor, which is what refuses it: {addresses:?}"
    );
}

struct FakeSolver;

#[async_trait]
impl rd_plugin_api::CaptchaSolver for FakeSolver {
    async fn solve(
        &self,
        challenge: CaptchaChallenge,
        _limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        match challenge {
            CaptchaChallenge::HCaptcha(widget) => {
                Ok(CaptchaAnswer::Token(format!("token:{}", widget.site_key)))
            }
            _ => Err(Failure::coded(
                FailureKind::Unsupported,
                "captcha.no_solver",
                "no solver for this kind",
            )),
        }
    }
}

fn challenge(kind: &str) -> CaptchaRequest {
    CaptchaRequest {
        challenge: kind.to_owned(),
        sitekey: Some("abc".to_owned()),
        page_url: "https://example.org/page".parse().expect("url"),
        invisible: false,
    }
}

/// A `form` step's fields go out as the browser's form encoding, or -- when the step says
/// `json` (RD-1170-03) -- as one JSON object; a request without fields carries no body unless
/// it is a JSON one, which is then `{}`.
#[test]
fn form_fields_travel_as_a_form_or_as_one_json_object() {
    let fields: BTreeMap<String, String> = [
        ("fphash".to_owned(), "0a1b".to_owned()),
        ("recaptchaToken".to_owned(), "t \"1\"&x".to_owned()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        super::encode_body(&fields, false),
        Some((
            "application/x-www-form-urlencoded",
            "fphash=0a1b&recaptchaToken=t+%221%22%26x".to_owned()
        ))
    );
    let (content_type, body) = super::encode_body(&fields, true).expect("a body");
    assert_eq!(content_type, "application/json");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(
        parsed,
        serde_json::json!({ "fphash": "0a1b", "recaptchaToken": "t \"1\"&x" })
    );
    assert_eq!(super::encode_body(&BTreeMap::new(), false), None);
    assert_eq!(
        super::encode_body(&BTreeMap::new(), true),
        Some(("application/json", "{}".to_owned()))
    );
}

/// A rule names a challenge kind and a site key, so only the widget kinds can cross. A kind
/// this does not know is refused rather than guessed at, and the broker's own refusal keeps
/// its code.
#[tokio::test]
async fn the_captcha_adapter_maps_the_widget_kinds_and_refuses_the_rest() {
    let adapter = RuleCaptcha::new(Arc::new(FakeSolver));
    assert_eq!(
        adapter.solve(challenge("hcaptcha")).await,
        Ok("token:abc".to_owned())
    );
    assert_eq!(
        adapter.solve(challenge("recaptcha-v2")).await,
        Err("captcha.no_solver".to_owned()),
        "the broker's stable code is what the run reports"
    );
    assert_eq!(
        adapter.solve(challenge("funcaptcha")).await,
        Err("no solver answers a funcaptcha challenge".to_owned())
    );
    let mut without_key = challenge("hcaptcha");
    without_key.sitekey = None;
    assert!(adapter.solve(without_key).await.is_err());
}
