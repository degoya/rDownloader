//! The local Click'n'Load listener.
//!
//! Two kinds of route live on this port and they do not get the same treatment (RD-109-01).
//!
//! `/flash`, `/jdcheck.js`, `/flash/add` and `/flash/addcrypted2` are JDownloader parity. A
//! hoster page calls them from its own origin, so an origin allowlist would lock out exactly
//! the pages the mechanism exists for; they therefore keep the permissive CORS headers, as a
//! named decision rather than an accident, and the README states the attack picture that comes
//! with it.
//!
//! `/rdownloader/nzb` is rDownloader's own route. Its only caller is this binary's `open`
//! subcommand behind the file association, which is not a browser and sends neither `Origin`
//! nor `Referer`. It therefore carries no `Access-Control-Allow-*` header at all and refuses a
//! request that carries either of those two headers.
//!
//! A refused request answers with a stable code and nothing else. The prose used to be the
//! response body, which handed a cross-origin caller a padding oracle against the AES path and
//! a readable copy of whatever the service had just said. The detail stays in the log.

use std::{collections::HashMap, net::SocketAddr, sync::LazyLock, time::Duration};

use aes::Aes128;
use anyhow::{Context, Result, bail};
use axum::{
    Form, Router,
    extract::{Multipart, Query, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use boa_engine::{Context as JsContext, Source};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use regex::Regex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;

use crate::client::CaptureClient;

const MAX_JK_BYTES: usize = 32 * 1024;
const MAX_CRYPTED_BYTES: usize = 8 * 1024 * 1024;
const MAX_JS_INSTRUCTIONS: usize = 100_000;

/// Largest request body `/flash/addcrypted2` accepts, refused before the form is decoded.
///
/// The limit that mattered used to sit inside the handler, and Axum had already decoded the
/// whole body into a `HashMap<String, String>` by the time it was reached — so a 65 MiB POST was
/// parsed in full and then measured against 8 MiB. The encrypted payload may be 8 MiB, base64
/// makes that about 10.7 MiB, and form encoding escapes some of those characters, so twenty is
/// the round number that leaves room for the payload, the `jk` script and the field names. The
/// exact 8 MiB check stays in the handler; this one keeps a page from making the agent decode
/// tens of megabytes before that check is reached.
const MAX_ADDCRYPTED_BODY_BYTES: usize = 20 * 1024 * 1024;

/// Largest request body `/flash/add` accepts. A plain list of links; a megabyte is thousands.
const MAX_ADD_BODY_BYTES: usize = 1024 * 1024;

/// Largest request body `/rdownloader/nzb` accepts: the NZB itself plus multipart framing.
const MAX_NZB_BODY_BYTES: usize = rd_collector::MAX_NZB_BYTES + 64 * 1024;

/// Largest request body the bodiless routes accept, so nothing on this port is unbounded.
const MAX_PROBE_BODY_BYTES: usize = 4 * 1024;

/// Why a payload with no usable link was refused.
///
/// Named after what `rd_collector::extract_urls` really takes. The message used to say "no
/// HTTP(S) links" while the extractor had long accepted magnets and the remote-transfer schemes
/// as well, so a payload of `ftp://` links was refused with a reason that was not the reason.
/// `SCHEMES_THE_COLLECTOR_TAKES` holds the same list for the test that keeps the two honest.
const NO_LINK_DETAIL: &str = "CNL payload contains no link the collector accepts (http, https, \
                              ftp, ftps, sftp, webdav, webdavs, dav, davs or magnet)";

/// How long the caller waits for a `jk` script to produce a key.
const JK_BUDGET: Duration = Duration::from_millis(250);

/// How many `jk` scripts may be evaluated at the same time.
///
/// Two, because a person answering a Click'n'Load button does it once; anything beyond that is
/// either a retry or a page trying to keep the agent busy. See [`JK_SLOTS`] for why the number
/// has to exist at all.
const MAX_CONCURRENT_JK: usize = 2;

/// The slots a `jk` evaluation runs in.
///
/// Boa has no interrupt hook, so a script that is still running cannot be stopped from outside;
/// its instruction budget ([`MAX_JS_INSTRUCTIONS`]) is what ends it. `tokio::time::timeout` only
/// ever ended the *waiting*, which is why this used to be a way to eat the runtime: each call
/// took a thread out of tokio's blocking pool — the same pool `arboard` reads the clipboard on
/// and `notify_rust` raises notifications on — and held it until the budget ran out.
///
/// Two things changed. The evaluation now runs on a thread of its own rather than on the
/// blocking pool, so an over-running script can no longer starve the clipboard or the
/// notifications; and this semaphore caps how many such threads can exist, so a page cannot
/// open one per request. A caller that finds no free slot is refused at once instead of queuing.
static JK_SLOTS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_JK);

/// A quoted 32-character hexadecimal literal: the static Click'n'Load key as a script writes it.
///
/// Anchored on the quotes on purpose. The pattern used to be a bare `([0-9a-f]{32})`, which
/// matches *anywhere*: an unrelated 32-digit identifier in the script, or the first half of a
/// 64-digit literal, silently became the key. Decryption then failed with "invalid CNL padding"
/// and nothing said the key had come from the wrong place. With the quotes required, a 64-digit
/// literal no longer matches at all, and more than one match is reported as ambiguous rather
/// than resolved by taking the first.
static QUOTED_STATIC_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"['"]([0-9a-fA-F]{32})['"]"#).expect("static CNL key regex"));

/// The first function declaration in a `jk` script, which is the one that is called.
///
/// `LazyLock` for the same reason as [`QUOTED_STATIC_KEY`] and as
/// `rd_collector::links::URL_PATTERN`: this sits in the request path, and recompiling a regex
/// per request is work a caller gets to ask for for free.
static JK_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(").expect("static CNL jk regex")
});

/// The codes a refused Click'n'Load request answers with.
///
/// Stable and deliberately uninformative: the caller learns that the request was refused and
/// which broad kind of refusal it was, and nothing about the state of the service or of the
/// decryption. Everything else goes to the log.
pub(crate) mod code {
    /// The payload could not be read: bad base64, bad padding, not UTF-8, malformed multipart.
    pub const INVALID_PAYLOAD: &str = "cnl_invalid_payload";
    /// No usable decryption key was supplied.
    pub const INVALID_KEY: &str = "cnl_invalid_key";
    /// The request carried nothing that could be handed to the LinkGrabber.
    pub const NO_LINKS: &str = "cnl_no_links";
    /// The payload exceeds what this endpoint accepts.
    pub const TOO_LARGE: &str = "cnl_payload_too_large";
    /// The service refused the hand-over, or could not be reached.
    pub const SERVICE_UNAVAILABLE: &str = "cnl_service_unavailable";
    /// A browser tried to reach one of rDownloader's own routes.
    pub const FOREIGN_ORIGIN: &str = "cnl_foreign_origin";
}

#[derive(Clone)]
struct CnlState {
    client: CaptureClient,
}

pub async fn serve(
    address: SocketAddr,
    listener: tokio::net::TcpListener,
    client: CaptureClient,
    cancellation: CancellationToken,
) -> Result<()> {
    axum::serve(listener, router(CnlState { client }))
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await
        .with_context(|| format!("Click'n'Load listener on {address} stopped"))
}

/// The router, built apart from the listener so the routing rules are testable on any host.
fn router(state: CnlState) -> Router {
    // JDownloader parity, cross-origin on purpose. `GET /flash/add` stays: a Click'n'Load
    // button on a hoster page is a plain navigation or image request from that page's own
    // origin, and dropping the method would break every existing button for no gain — the POST
    // form next to it is reachable from the same page without a preflight either.
    // Every route carries the limit that belongs to it, rather than all of them sharing the
    // 65 MiB the largest one needs (RD-109-02). `RequestBodyLimitLayer` refuses on the declared
    // length before the extractor sees the body, so the check happens before the decoding.
    let flash = Router::new()
        .route(
            "/flash",
            get(check)
                .options(preflight)
                .layer(RequestBodyLimitLayer::new(MAX_PROBE_BODY_BYTES)),
        )
        .route(
            "/jdcheck.js",
            get(check_script)
                .options(preflight)
                .layer(RequestBodyLimitLayer::new(MAX_PROBE_BODY_BYTES)),
        )
        .route(
            "/flash/add",
            get(add_query)
                .post(add)
                .options(preflight)
                .layer(RequestBodyLimitLayer::new(MAX_ADD_BODY_BYTES)),
        )
        .route(
            "/flash/addcrypted2",
            post(add_crypted)
                .options(preflight)
                .layer(RequestBodyLimitLayer::new(MAX_ADDCRYPTED_BODY_BYTES)),
        )
        .layer(middleware::from_fn(flash_cors_headers));
    // rDownloader's own route: no CORS headers, and no browser may reach it.
    let own = Router::new()
        .route(
            "/rdownloader/nzb",
            post(agent_nzb).layer(RequestBodyLimitLayer::new(MAX_NZB_BODY_BYTES)),
        )
        .layer(middleware::from_fn(refuse_browser_callers));
    flash.merge(own).with_state(state)
}

async fn add_query(
    State(state): State<CnlState>,
    Query(fields): Query<HashMap<String, String>>,
) -> Result<&'static str, CnlError> {
    add_fields(&state.client, &fields).await
}

async fn agent_nzb(
    State(state): State<CnlState>,
    mut multipart: Multipart,
) -> Result<&'static str, CnlError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| CnlError::new(code::INVALID_PAYLOAD, error))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("association.nzb").to_owned();
        let content = field
            .bytes()
            .await
            .map_err(|error| CnlError::new(code::INVALID_PAYLOAD, error))?;
        if content.len() > rd_collector::MAX_NZB_BYTES {
            return Err(CnlError::new(
                code::TOO_LARGE,
                anyhow::anyhow!("NZB exceeds 64 MiB"),
            ));
        }
        state
            .client
            .upload_nzb_bytes(name, content.to_vec())
            .await
            .map_err(|error| CnlError::new(code::SERVICE_UNAVAILABLE, error))?;
        return Ok("success");
    }
    Err(CnlError::new(
        code::INVALID_PAYLOAD,
        anyhow::anyhow!("multipart field 'file' is missing"),
    ))
}

async fn check() -> &'static str {
    "JDownloader"
}

async fn check_script() -> &'static str {
    "jdownloader=true; var version='rDownloader';"
}

/// Answers a CORS preflight for a route that exists.
///
/// Registered per route rather than as a fallback: the fallback used to answer `OPTIONS` on
/// every path with 200, including paths this server has never served.
async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn add(
    State(state): State<CnlState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<&'static str, CnlError> {
    add_fields(&state.client, &fields).await
}

async fn add_fields(
    client: &CaptureClient,
    fields: &HashMap<String, String>,
) -> Result<&'static str, CnlError> {
    let text = fields
        .get("urls")
        .or_else(|| fields.get("url"))
        .ok_or_else(|| {
            CnlError::new(
                code::NO_LINKS,
                anyhow::anyhow!("CNL request contains no URLs"),
            )
        })?;
    submit(client, text, fields).await?;
    Ok("success")
}

async fn add_crypted(
    State(state): State<CnlState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<&'static str, CnlError> {
    let encrypted = fields.get("crypted").ok_or_else(|| {
        CnlError::new(
            code::INVALID_PAYLOAD,
            anyhow::anyhow!("CNL request contains no encrypted payload"),
        )
    })?;
    let key_source = fields
        .get("key")
        .or_else(|| fields.get("jk"))
        .ok_or_else(|| {
            CnlError::new(
                code::INVALID_KEY,
                anyhow::anyhow!("CNL request contains no key"),
            )
        })?;
    let key = resolve_key(key_source)
        .await
        .map_err(|error| CnlError::new(code::INVALID_KEY, error))?;
    if encrypted.len() > MAX_CRYPTED_BYTES * 2 {
        return Err(CnlError::new(
            code::TOO_LARGE,
            anyhow::anyhow!("CNL encrypted payload exceeds its size limit"),
        ));
    }
    let mut payload = STANDARD
        .decode(encrypted.trim())
        .map_err(|error| CnlError::new(code::INVALID_PAYLOAD, error))?;
    if payload.len() > MAX_CRYPTED_BYTES {
        return Err(CnlError::new(
            code::TOO_LARGE,
            anyhow::anyhow!("CNL encrypted payload exceeds its size limit"),
        ));
    }
    let decrypted = Decryptor::<Aes128>::new_from_slices(&key, &key)
        .map_err(|error| CnlError::new(code::INVALID_KEY, anyhow::Error::new(error)))?
        .decrypt_padded_mut::<Pkcs7>(&mut payload)
        .map_err(|_| {
            CnlError::new(
                code::INVALID_PAYLOAD,
                anyhow::anyhow!("invalid CNL padding"),
            )
        })?;
    let text = std::str::from_utf8(decrypted)
        .map_err(|error| CnlError::new(code::INVALID_PAYLOAD, error))?;
    submit(&state.client, text, &fields).await?;
    Ok("success")
}

/// Forwards the CNL `package` name and the first line of `passwords` with the links.
async fn submit(
    client: &CaptureClient,
    text: &str,
    fields: &HashMap<String, String>,
) -> Result<(), CnlError> {
    let urls = rd_collector::extract_urls(text);
    if urls.is_empty() {
        return Err(CnlError::new(
            code::NO_LINKS,
            anyhow::anyhow!(NO_LINK_DETAIL),
        ));
    }
    let package = fields
        .get("package")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let password = fields
        .get("passwords")
        .and_then(|value| value.lines().map(str::trim).find(|line| !line.is_empty()));
    client
        .submit_links(urls, "click_and_load", package, password)
        .await
        .map_err(|error| CnlError::new(code::SERVICE_UNAVAILABLE, error))
}

/// Reads the key straight out of the `jk` field, when it is unambiguously there.
///
/// Three outcomes, and the middle one is the point of this function (RD-109-02):
///
/// - `Ok(Some(key))` — the field is the key itself, or the script contains exactly one quoted
///   32-digit hexadecimal literal.
/// - `Ok(None)` — there is no literal key; the script has to be evaluated.
/// - `Err(..)` — the script contains more than one candidate. That is refused rather than
///   resolved by taking the first, which is what the old unanchored pattern did.
fn extract_static_key(source: &str) -> Result<Option<[u8; 16]>> {
    let trimmed = source.trim();
    // The `key` form: the field is the key, nothing else.
    if trimmed.len() == 32 && trimmed.chars().all(|value| value.is_ascii_hexdigit()) {
        return decode_key(trimmed).map(Some);
    }
    let mut candidates: Vec<String> = QUOTED_STATIC_KEY
        .captures_iter(trimmed)
        .filter_map(|capture| capture.get(1))
        .map(|value| value.as_str().to_ascii_lowercase())
        .collect();
    candidates.dedup();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.len() {
        0 => Ok(None),
        1 => decode_key(&candidates[0]).map(Some),
        count => bail!(
            "CNL jk contains {count} different 128-bit hexadecimal literals; which one is the \
             key cannot be guessed"
        ),
    }
}

async fn resolve_key(source: &str) -> Result<[u8; 16]> {
    resolve_key_within(source, JK_BUDGET).await
}

/// The body of [`resolve_key`], with the budget passed in so a test can drive the expiry.
async fn resolve_key_within(source: &str, budget: Duration) -> Result<[u8; 16]> {
    if let Some(key) = extract_static_key(source)? {
        return Ok(key);
    }
    if source.len() > MAX_JK_BYTES {
        bail!("CNL jk script exceeds its size limit");
    }
    // `try_acquire`, not `acquire`: a caller that finds every slot taken is told so now rather
    // than joining a queue that a page could make arbitrarily long.
    let permit = JK_SLOTS
        .try_acquire()
        .map_err(|_| anyhow::anyhow!("CNL jk evaluation slots are all busy"))?;
    let source = source.to_owned();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // A thread of this crate's own, never tokio's blocking pool: an evaluation that outlives
    // its budget then costs one thread that nothing else wanted, instead of one the clipboard
    // and the desktop notifications were going to need.
    std::thread::Builder::new()
        .name("cnl-jk".to_owned())
        .spawn(move || {
            // Released when the thread really ends, not when the caller stops waiting — so the
            // cap counts scripts that are still running, which is the thing worth capping.
            let _permit = permit;
            let _ = sender.send(evaluate_jk(&source));
        })
        .context("start the CNL jk sandbox thread")?;
    let result = tokio::time::timeout(budget, receiver)
        .await
        .context("CNL jk execution timed out")?
        .context("CNL jk sandbox ended without a result")??;
    decode_key(&result)
}

fn evaluate_jk(source: &str) -> Result<String> {
    let function = JK_FUNCTION
        .captures(source)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str())
        .context("CNL jk script declares no callable function")?;
    let program = format!("\"use strict\";\n{source}\n{function}();");
    let mut context = JsContext::builder()
        .instructions_remaining(MAX_JS_INSTRUCTIONS)
        .can_block(false)
        .build()
        .map_err(|error| anyhow::anyhow!("create CNL JavaScript sandbox: {error}"))?;
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(10_000);
    context.runtime_limits_mut().set_recursion_limit(64);
    context.runtime_limits_mut().set_stack_size_limit(1024);
    let value = context
        .eval(Source::from_bytes(program.as_bytes()))
        .map_err(|error| anyhow::anyhow!("evaluate CNL jk script: {error}"))?;
    let value = value
        .to_string(&mut context)
        .map_err(|error| anyhow::anyhow!("convert CNL jk result: {error}"))?
        .to_std_string_escaped();
    if value.len() > 128 {
        bail!("CNL jk result exceeds its size limit");
    }
    Ok(value)
}

fn decode_key(encoded: &str) -> Result<[u8; 16]> {
    let encoded = encoded.trim();
    if encoded.len() != 32
        || !encoded
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        bail!("CNL jk result is not a 128-bit hexadecimal key");
    }
    hex::decode(encoded)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid CNL key length"))
}

/// Puts the permissive headers on the JDownloader-compatible routes, and only on those.
async fn flash_cors_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    headers.insert(
        "access-control-allow-private-network",
        HeaderValue::from_static("true"),
    );
    response
}

/// Refuses a request to rDownloader's own routes that came from a page.
///
/// `Origin` and `Referer` are set by a browser and by nothing else the agent talks to: the only
/// caller of this route is the `open` subcommand in this same binary, which sends neither. So
/// the presence of either header is enough to know the request is not the one this route exists
/// for, and it is refused before the body is touched.
async fn refuse_browser_callers(request: axum::extract::Request, next: Next) -> Response {
    let headers = request.headers();
    if headers.contains_key(header::ORIGIN) || headers.contains_key(header::REFERER) {
        tracing::warn!(
            path = %request.uri().path(),
            "refused a cross-origin call to an agent-only Click'n'Load route"
        );
        return CnlError::new(
            code::FOREIGN_ORIGIN,
            anyhow::anyhow!("agent-only route is not reachable from a page"),
        )
        .with_status(StatusCode::FORBIDDEN)
        .into_response();
    }
    next.run(request).await
}

/// A refused Click'n'Load request.
///
/// The body is the code and nothing else. `source` never leaves the process: it is logged here
/// and dropped, which is what keeps `invalid CNL padding` from being an oracle and a
/// `ServiceRefusal` from being readable by whatever page made the call.
struct CnlError {
    status: StatusCode,
    code: &'static str,
    source: anyhow::Error,
}

impl CnlError {
    fn new(code: &'static str, source: impl Into<anyhow::Error>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            source: source.into(),
        }
    }

    fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }
}

impl IntoResponse for CnlError {
    fn into_response(self) -> Response {
        tracing::warn!(code = self.code, error = %self.source, "Click'n'Load request rejected");
        (self.status, self.code).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use axum::http::{StatusCode, header};
    use tokio_util::sync::CancellationToken;
    use url::Url;

    use super::{
        CnlState, JK_SLOTS, MAX_ADDCRYPTED_BODY_BYTES, MAX_CONCURRENT_JK, NO_LINK_DETAIL, code,
        extract_static_key, resolve_key, resolve_key_within, router,
    };
    use crate::client::CaptureClient;

    /// For tests that expect an evaluation to finish. The production budget of 250 ms was
    /// overrun by Boa's first evaluation on a loaded GitHub runner (2026-09-25), which turned a
    /// test of the result into a test of the machine; the expiry has its own tests.
    async fn resolve_patiently(source: &str) -> anyhow::Result<[u8; 16]> {
        resolve_key_within(source, Duration::from_secs(10)).await
    }

    /// A live listener on an ephemeral port, so the routing rules are exercised the way a
    /// browser would meet them rather than through a hand-built request.
    ///
    /// The client points at a port nothing listens on: every test here is about a request that
    /// is refused before anything is handed over, and a hand-over that did happen would fail
    /// loudly rather than reach a real service.
    async fn spawn() -> (SocketAddr, CancellationToken) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let address = listener.local_addr().expect("the bound address");
        let cancellation = CancellationToken::new();
        let state = CnlState {
            client: CaptureClient::new(
                Url::parse("http://127.0.0.1:9/").expect("valid URL"),
                "x".repeat(32),
            )
            .expect("build a capture client"),
        };
        let shutdown = cancellation.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router(state))
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        (address, cancellation)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build a client")
    }

    #[test]
    fn accepts_literal_or_javascript_wrapped_static_keys() {
        let expected = [0xab_u8; 16];
        assert_eq!(
            extract_static_key("function f(){ return 'abababababababababababababababab'; }")
                .expect("one unambiguous literal"),
            Some(expected)
        );
        // The `key` form: the field is the key itself.
        assert_eq!(
            extract_static_key("  abababababababababababababababab  ").expect("a bare key"),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn evaluates_non_literal_key_in_boa_without_host_apis() {
        let script =
            "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
        assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
    }

    #[tokio::test]
    async fn rejects_unbounded_javascript_loops() {
        let script = "function getKey(){ while(true){} }";
        assert!(resolve_key(script).await.is_err());
    }

    /// The agent's own NZB route is not a browser route. A page that finds the port must not be
    /// able to push a 64 MiB NZB of its choosing into the LinkGrabber.
    #[tokio::test]
    async fn the_agent_nzb_route_refuses_a_foreign_origin_and_offers_no_cors() {
        let (address, cancellation) = spawn().await;
        let endpoint = format!("http://{address}/rdownloader/nzb");

        for (name, value) in [
            (header::ORIGIN, "https://evil.test"),
            (header::REFERER, "https://evil.test/page"),
        ] {
            let response = client()
                .post(&endpoint)
                .header(name.clone(), value)
                .body("whatever")
                .send()
                .await
                .expect("the listener answers");
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "a request carrying {name} must not reach the route"
            );
            assert!(
                response
                    .headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none(),
                "the agent's own route must not advertise cross-origin access"
            );
            assert_eq!(response.text().await.expect("a body"), code::FOREIGN_ORIGIN);
        }

        // Without those headers the route is reachable again — the refusal is about the caller,
        // not about the route being switched off. The multipart body is missing, so this ends
        // in the payload refusal rather than in a hand-over.
        let response = client()
            .post(&endpoint)
            .body("not multipart")
            .send()
            .await
            .expect("the listener answers");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "no answer from this route carries a CORS header"
        );
        cancellation.cancel();
    }

    /// The JDownloader-compatible routes keep their wildcard, as the documented exception.
    #[tokio::test]
    async fn the_flash_routes_keep_their_documented_wildcard() {
        let (address, cancellation) = spawn().await;
        let response = client()
            .get(format!("http://{address}/flash"))
            .header(header::ORIGIN, "https://hoster.test")
            .send()
            .await
            .expect("the listener answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-private-network")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        cancellation.cancel();
    }

    /// The body of a refusal is a code. Neither the decryption's own account of itself nor
    /// anything the service said may be readable by the page that made the call.
    #[tokio::test]
    async fn a_refusal_answers_with_a_code_and_no_prose() {
        let (address, cancellation) = spawn().await;
        let response = client()
            .post(format!("http://{address}/flash/addcrypted2"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(
                "crypted=bm90IHBhZGRlZCBjb3JyZWN0bHkh\
                 &jk=function%20f()%7B%20return%20%27abababababababababababababababab%27%3B%20%7D",
            )
            .send()
            .await
            .expect("the listener answers");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.text().await.expect("a body");
        assert_eq!(body, code::INVALID_PAYLOAD);
        assert!(
            !body.contains("invalid CNL padding"),
            "the padding oracle must not be readable: {body}"
        );

        // The same for a hand-over the service refused: the agent's account of it stays in the
        // log. Nothing listens on the service port here, so this is the unreachable case.
        let response = client()
            .post(format!("http://{address}/flash/add"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body("urls=https%3A%2F%2Fexample.com%2Ffile.bin")
            .send()
            .await
            .expect("the listener answers");
        let body = response.text().await.expect("a body");
        assert_eq!(body, code::SERVICE_UNAVAILABLE);
        assert!(
            !body.contains("127.0.0.1"),
            "nothing about the service may leak into the answer: {body}"
        );
        cancellation.cancel();
    }

    /// `crossdomain.xml` is gone, and `OPTIONS` no longer says yes to paths that do not exist.
    #[tokio::test]
    async fn only_paths_that_exist_answer_at_all() {
        let (address, cancellation) = spawn().await;
        for path in ["/crossdomain.xml", "/nothing/here"] {
            for method in [reqwest::Method::GET, reqwest::Method::OPTIONS] {
                let response = client()
                    .request(method.clone(), format!("http://{address}{path}"))
                    .send()
                    .await
                    .expect("the listener answers");
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{method} {path} must not be answered"
                );
            }
        }
        // A path that does exist still answers its preflight.
        let response = client()
            .request(
                reqwest::Method::OPTIONS,
                format!("http://{address}/flash/add"),
            )
            .send()
            .await
            .expect("the listener answers");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        cancellation.cancel();
    }

    /// The limit that matters has to be reached before the body is decoded, not after. A 413
    /// rather than the handler's 400 is what tells the two apart.
    ///
    /// Only the declared length is sent, over a raw socket. The listener answers on the header
    /// and closes without reading the body, and a client still writing 20 MiB into that socket
    /// sees a reset instead of the answer on Windows (os error 10053, CI on 2026-09-25) — the
    /// refusal the test is about happened, the test just could not read it.
    #[tokio::test]
    async fn an_oversized_addcrypted_body_is_refused_before_it_is_decoded() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (address, cancellation) = spawn().await;
        let mut socket = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect to the listener");
        let request = format!(
            "POST /flash/addcrypted2 HTTP/1.1\r\nHost: {address}\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\ncrypted=",
            MAX_ADDCRYPTED_BODY_BYTES + 16
        );
        socket
            .write_all(request.as_bytes())
            .await
            .expect("send the request head");
        let mut answer = Vec::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut answer)).await;
        let status_line = String::from_utf8_lossy(&answer)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        assert!(
            status_line.starts_with("HTTP/1.1 413"),
            "the body limit has to fire before the form extractor runs, got {status_line:?}"
        );
        cancellation.cancel();
    }

    /// The timeout used to end only the waiting. What it has to end now is the claim on the
    /// runtime: the agent is still able to evaluate the next script right afterwards.
    #[tokio::test]
    async fn an_expired_jk_evaluation_leaves_the_agent_working() {
        let spinning = "function getKey(){ var n=0; while(true){ n=n+1; } }";
        let expired = resolve_key_within(spinning, Duration::from_millis(1)).await;
        assert!(expired.is_err(), "a script that never returns must not win");

        // The next caller is served, which is the property the blocking pool used to lose.
        let script =
            "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
        assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
    }

    /// A page must not be able to open one evaluation per request.
    #[tokio::test]
    async fn concurrent_jk_evaluations_are_capped() {
        let held = JK_SLOTS
            .try_acquire_many(u32::try_from(MAX_CONCURRENT_JK).expect("a small cap"))
            .expect("every slot is free at the start of this test");
        let script =
            "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
        let refused = resolve_key(script).await;
        assert!(
            refused.is_err(),
            "with every slot taken the call has to be refused, not queued"
        );
        drop(held);
        assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
    }

    /// The old pattern matched anywhere, so an unrelated identifier or the first half of a
    /// 64-digit literal silently became the key and decryption then failed with a padding
    /// error that pointed at nothing.
    #[tokio::test]
    async fn an_ambiguous_static_key_is_refused_rather_than_guessed_at() {
        let two_literals = "function getKey(){ var id='0123456789abcdef0123456789abcdef'; \
                            return 'abababababababababababababababab'; }";
        assert!(
            extract_static_key(two_literals).is_err(),
            "two candidates cannot be resolved by taking the first"
        );
        assert!(resolve_key(two_literals).await.is_err());

        let sixty_four = format!("function getKey(){{ return '{}'; }}", "ab".repeat(32));
        assert_eq!(
            extract_static_key(&sixty_four).expect("no literal key, not an error"),
            None,
            "a 64-digit literal is not a 128-bit key and must not be cut in half"
        );
        // It is then evaluated, and the 64-digit result is refused with a reason.
        let error = resolve_patiently(&sixty_four)
            .await
            .expect_err("a 64-digit result is not a key");
        assert!(
            error.to_string().contains("128-bit hexadecimal key"),
            "{error}"
        );
    }

    /// The refusal used to name HTTP(S) only, while the collector had long taken more.
    #[test]
    fn the_link_free_refusal_names_the_schemes_that_are_really_accepted() {
        for sample in [
            "http://example.com/a",
            "https://example.com/a",
            "ftp://example.com/a",
            "ftps://example.com/a",
            "sftp://example.com/a",
            "webdav://example.com/a",
            "webdavs://example.com/a",
            "dav://example.com/a",
            "davs://example.com/a",
            "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01",
        ] {
            let scheme = sample.split([':', '/']).next().expect("a scheme");
            assert!(
                NO_LINK_DETAIL.contains(scheme),
                "the refusal has to name {scheme}, which the collector accepts"
            );
            assert!(
                !rd_collector::extract_urls(sample).is_empty(),
                "the refusal names {scheme}, so the collector has to take it: {sample}"
            );
        }
    }
}
