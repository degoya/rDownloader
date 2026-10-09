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
//! rDownloader's own route `/rdownloader/nzb` is gone (RD-1200-03): it took an NZB from anyone
//! who reached the port and handed it over with the agent's token, and its one caller, `open`,
//! now hands the file to the service itself, with the pairing token.
//!
//! A refused request answers with a stable code and nothing else. The prose used to be the
//! response body, which handed a cross-origin caller a padding oracle against the AES path and
//! a readable copy of whatever the service had just said. The detail stays in the log.

use std::{collections::HashMap, net::SocketAddr};

use aes::Aes128;
use anyhow::{Context, Result};
use axum::{
    Form, Router,
    extract::{Query, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7},
};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;

use crate::client::CaptureClient;

mod jk;
mod pace;

use jk::resolve_key;
use pace::Pace;

const MAX_CRYPTED_BYTES: usize = 8 * 1024 * 1024;

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

/// The codes a refused Click'n'Load request answers with.
///
/// Stable and deliberately uninformative: the caller learns that the request was refused and
/// which broad kind of refusal it was, and nothing about the state of the service or of the
/// decryption. Everything else goes to the log.
pub(crate) mod code {
    /// The payload could not be read: bad base64, bad padding, not UTF-8, malformed multipart.
    pub(crate) const INVALID_PAYLOAD: &str = "cnl_invalid_payload";
    /// No usable decryption key was supplied.
    pub(crate) const INVALID_KEY: &str = "cnl_invalid_key";
    /// The request carried nothing that could be handed to the LinkGrabber.
    pub(crate) const NO_LINKS: &str = "cnl_no_links";
    /// The payload exceeds what this endpoint accepts.
    pub(crate) const TOO_LARGE: &str = "cnl_payload_too_large";
    /// The service refused the hand-over, or could not be reached.
    pub(crate) const SERVICE_UNAVAILABLE: &str = "cnl_service_unavailable";
    /// More hand-overs than the agent accepts per minute (RD-1190-22).
    pub(crate) const RATE_LIMITED: &str = "cnl_rate_limited";
}

#[derive(Clone)]
struct CnlState {
    client: CaptureClient,
    pace: std::sync::Arc<Pace>,
}

pub(crate) async fn serve(
    address: SocketAddr,
    listener: tokio::net::TcpListener,
    client: CaptureClient,
    cancellation: CancellationToken,
) -> Result<()> {
    let state = CnlState {
        client,
        pace: std::sync::Arc::clone(&pace::SHARED),
    };
    axum::serve(listener, router(state))
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
    flash.with_state(state)
}

async fn add_query(
    State(state): State<CnlState>,
    Query(fields): Query<HashMap<String, String>>,
) -> Result<&'static str, CnlError> {
    add_fields(&state, &fields).await
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
    add_fields(&state, &fields).await
}

async fn add_fields(
    state: &CnlState,
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
    submit(state, text, fields).await?;
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
        .decrypt_padded::<Pkcs7>(&mut payload)
        .map_err(|_| {
            CnlError::new(
                code::INVALID_PAYLOAD,
                anyhow::anyhow!("invalid CNL padding"),
            )
        })?;
    let text = std::str::from_utf8(decrypted)
        .map_err(|error| CnlError::new(code::INVALID_PAYLOAD, error))?;
    submit(&state, text, &fields).await?;
    Ok("success")
}

/// Forwards the CNL `package` name and the first line of `passwords` with the links, within
/// the pace and the field lengths of [`pace`].
async fn submit(
    state: &CnlState,
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
    let package = pace::package_name(fields.get("package").map(String::as_str));
    let password =
        pace::password(fields.get("passwords").map(String::as_str)).map_err(|length| {
            CnlError::new(
                code::TOO_LARGE,
                anyhow::anyhow!("CNL password of {length} characters exceeds its limit"),
            )
        })?;
    if !state.pace.admit(std::time::Instant::now()) {
        return Err(CnlError::new(
            code::RATE_LIMITED,
            anyhow::anyhow!("more Click'n'Load hand-overs than the agent accepts per minute"),
        )
        .with_status(StatusCode::TOO_MANY_REQUESTS));
    }
    let submitted = state
        .client
        .submit_links(urls, "click_and_load", package.as_deref(), password)
        .await
        .map_err(|error| CnlError::new(code::SERVICE_UNAVAILABLE, error))?;
    // A page whose releases wait for a choice (RD-1190-17) is a success the page that sent the
    // links hears as one; the person hears where to choose.
    if let Some(notice) = submitted.notice() {
        crate::notify::toast(notice).await;
    }
    Ok(())
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
    // `X-Referer`: hide.cx posts to `/flash/add` with `fetch()` and that header, so the browser
    // asks first and drops the post when the preflight does not name it (owner, 2026-10-07).
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, X-Referer"),
    );
    headers.insert(
        "access-control-allow-private-network",
        HeaderValue::from_static("true"),
    );
    response
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
mod tests;
