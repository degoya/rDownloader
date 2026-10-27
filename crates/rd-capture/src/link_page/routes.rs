//! The link page's two routes, `GET` and `POST /link/<secret>`, and the rules every answer of
//! theirs obeys (see the module head of `link_page`).

use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use tokio::sync::mpsc;
use tower_http::limit::RequestBodyLimitLayer;
use url::Url;

use super::{
    LinkRefusal, LinkRequest, LinkSecret, Linked, host_allowed, origin_allowed, secret_matches,
    verify_and_save,
};
use crate::config;

/// The page: one file, no asset of its own, its words and the stored address put in on the way.
const PAGE: &str = include_str!("../../assets/link.html");

/// Largest body the `POST` takes: an address and a token are a few hundred bytes.
const MAX_LINK_BODY_BYTES: usize = 16 * 1024;

/// The page runs its own inline script and style and talks to its own address, nothing else; no
/// form goes anywhere and no other page may frame it.
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; \
                                       script-src 'unsafe-inline'; connect-src 'self'; \
                                       form-action 'none'; frame-ancestors 'none'";

/// Stores a verified link: `config::save` in the agent, a recorder in the tests.
pub(crate) type Store = Arc<dyn Fn(&Url, &str, bool) -> anyhow::Result<()> + Send + Sync>;

/// What the routes share: the secret, the port the `Host` and `Origin` have to name, where a
/// made link is announced, and where the address the page shows first is read.
#[derive(Clone)]
pub(crate) struct LinkState {
    pub secret: Arc<LinkSecret>,
    pub port: u16,
    pub linked: mpsc::UnboundedSender<Linked>,
    /// The configuration directory, for the stored address the page starts with.
    pub directory: PathBuf,
    pub store: Store,
}

/// The routes, built apart from the listener so the rules are testable on any host.
pub(crate) fn router(state: LinkState) -> Router {
    // The last layer is the outermost: the headers go on everything, the guard's 404 included.
    Router::new()
        .route("/link/{secret}", get(page).post(link))
        .layer(RequestBodyLimitLayer::new(MAX_LINK_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .layer(middleware::from_fn(protective_headers))
        .with_state(state)
}

/// A foreign `Host` or a wrong secret: 404 without a body, like a route that does not exist.
async fn guard(State(state): State<LinkState>, request: Request, next: Next) -> Response {
    let host = host_allowed(request.headers().get(header::HOST), state.port);
    let secret = request
        .uri()
        .path()
        .strip_prefix("/link/")
        .is_some_and(|given| secret_matches(given, &state.secret));
    if !(host && secret) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

/// The page, with its words in the agent's language and the stored address. Never a token: the
/// page does not read one, and a `GET` changes nothing, whatever its query says.
async fn page(State(state): State<LinkState>) -> Html<String> {
    let (lang, texts) = crate::notify::link_page_texts();
    let stored = config::stored_service_in(&state.directory, None);
    // Ticked from the start only for the address that was confirmed before: the stored one.
    let confirmed = stored
        .as_ref()
        .is_some_and(|service| config::ensure_transport_is_safe(service, false).is_err());
    let data = serde_json::json!({
        "texts": texts,
        "service": stored.as_ref().map(Url::as_str).unwrap_or_default(),
        "confirmed": confirmed,
    });
    Html(
        PAGE.replace("__LANG__", lang)
            .replace("__DATA__", &script_safe(&data.to_string())),
    )
}

/// Verifies and stores a link sent from the page itself, and announces it.
async fn link(State(state): State<LinkState>, headers: HeaderMap, body: Bytes) -> Response {
    if !origin_allowed(headers.get(header::ORIGIN), state.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(request) = serde_json::from_slice::<LinkRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let store = state.store.clone();
    let stored = verify_and_save(request, |service: &Url, token: &str, insecure: bool| {
        store(service, token, insecure)
    })
    .await;
    match stored {
        Ok(linked) => {
            let answer = serde_json::json!({
                "service": linked.service.as_str(),
                "insecure": linked.insecure,
            });
            if state.linked.send(linked).is_err() {
                tracing::warn!("the link was stored, but the agent no longer listens for it");
            }
            Json(answer).into_response()
        }
        Err(refusal) => {
            let status = match refusal {
                LinkRefusal::InvalidAddress | LinkRefusal::InsecureNotConfirmed => {
                    StatusCode::BAD_REQUEST
                }
                LinkRefusal::Unreachable | LinkRefusal::NotRdownloader => StatusCode::BAD_GATEWAY,
                LinkRefusal::TokenRejected => StatusCode::UNAUTHORIZED,
                LinkRefusal::MissingScope => StatusCode::FORBIDDEN,
                LinkRefusal::NotStored => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(serde_json::json!({ "code": refusal.code() }))).into_response()
        }
    }
}

/// What every answer of the page's routes carries.
async fn protective_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// JSON that can stand inside a `<script>` element: no `<` that could close it.
fn script_safe(json: &str) -> String {
    json.replace('<', "\\u003c")
}
