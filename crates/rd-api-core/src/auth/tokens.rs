//! Bearers: session and API tokens, the compatibility adapters' access and the capture token.

use super::*;

/// What opening a session produced: the bearer to hand back, and the id to audit under.
///
/// The id is returned rather than looked up again because the audit record of a sign-in has
/// to name the session that sign-in created, and a second lookup by digest would be a second
/// answer to the same question (RD-110-03).
pub struct OpenedSession {
    pub token: String,
    pub id: rd_core::SessionId,
    /// How long the browser keeps the cookie: the maximum lifetime at sign-in (RD-130-09).
    pub max_age_seconds: u64,
}

/// The stored form of a bearer: hex SHA-256, never the value itself.
pub(crate) fn digest_of(token: &str) -> String {
    rd_authn::sha256_hex(token)
}

/// The stored digest of this request's session credential, if it carries one.
pub fn session_digest(headers: &HeaderMap) -> Option<String> {
    session_token(headers).map(digest_of)
}

/// The session credential, from the bearer header or the cookie.
///
/// Bearer wins, as it did before: a command-line client that sets both should get the one it
/// chose deliberately rather than a cookie a browser left behind.
pub(crate) fn session_token(headers: &HeaderMap) -> Option<&str> {
    bearer_token(headers).or_else(|| cookie_token(headers))
}

/// Records that a machine token was just used, without making the request wait for it.
///
/// The session inventory shows a token's "last used", and for every machine token it stayed
/// empty: the column, its once-a-minute throttle and the writer command all existed, and
/// nothing ever called them. This is the one place every accepted bearer passes through, so it
/// is the only place the field can be filled without adding a second idea of what "accepted"
/// means.
///
/// Detached on purpose. The write goes through the serialized database writer, and a request
/// must not queue behind it for a field nobody reads in real time. The statement throttles
/// itself to once a minute, but only after the command has queued; [`TOKEN_TOUCHED`] keeps a
/// busy client -- an MCP tool call passed through here three times -- from queueing one
/// command per request in the first place (audit 1.9.1, API-14).
pub(super) fn note_token_use(state: &AppState, digest: String) {
    {
        let Ok(mut touched) = TOKEN_TOUCHED.lock() else {
            return;
        };
        if !touch_due(&mut touched, &digest, Instant::now()) {
            return;
        }
    }
    let database = state.database.clone();
    tokio::spawn(async move {
        if let Err(error) = database.touch_capture_token(digest).await {
            tracing::debug!(error = %error, "a machine token's last use was not recorded");
        }
    });
}

/// Whether `digest` is due its next "last used" write at `now`, noted as queued when it is: once
/// per [`rd_db::SESSION_TOUCH_INTERVAL_SECONDS`] for each token.
pub(super) fn touch_due(
    touched: &mut std::collections::HashMap<String, Instant>,
    digest: &str,
    now: Instant,
) -> bool {
    let interval = std::time::Duration::from_secs(
        u64::try_from(rd_db::SESSION_TOUCH_INTERVAL_SECONDS).unwrap_or(60),
    );
    if touched
        .get(digest)
        .is_some_and(|last| now.duration_since(*last) < interval)
    {
        return false;
    }
    // Bounded by the live tokens in practice; the sweep only keeps revoked ones from
    // lingering forever.
    if touched.len() >= 1024 {
        touched.retain(|_, last| now.duration_since(*last) < interval);
    }
    touched.insert(digest.to_owned(), now);
    true
}

/// When each token digest last queued its "last used" write, for [`note_token_use`].
pub(super) static TOKEN_TOUCHED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Instant>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// The scopes a SABnzbd or qBittorrent client needs (audit 1.9.1, API-04; owner, 2026-10-04):
/// it adds work, controls the work it added and reads the queue back -- and touches no setting,
/// no stored credential and no administration.
pub const COMPAT_SCOPES: &[rd_core::Scope] = &[
    rd_core::Scope::Intake,
    rd_core::Scope::Queue,
    rd_core::Scope::Read,
];

/// How a compatibility client's key fared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatAccess {
    /// A live token holding [`COMPAT_SCOPES`]; its use is recorded.
    Granted,
    /// No key, an unknown or revoked one, or one without the scopes.
    Refused,
    /// The token store could not be read: a fault of this service, not of the key.
    Unavailable,
}

/// The one credential check of the SABnzbd and qBittorrent adapters (API-04, API-11).
///
/// Both used to demand the literal `api:*` -- secrets and administration included -- and wrote
/// neither the token's "last used" nor an audit record, so a key handed to Sonarr was the most
/// powerful credential the service issues and the least visible one. Now it needs what the
/// adapters do and nothing more, and is recorded like every other bearer. An `api:*` token
/// still passes: it expands to every scope.
///
/// A database error is reported as [`CompatAccess::Unavailable`] instead of a refusal, so the
/// adapter can say "try later" rather than "wrong key" -- a client told its key is wrong asks
/// its user for a new one.
pub async fn compat_access(state: &AppState, key: &str) -> CompatAccess {
    if key.is_empty() {
        return CompatAccess::Refused;
    }
    let digest = digest_of(key);
    match state.database.capture_token_identity(&digest).await {
        Ok(Some((id, label, scopes))) => {
            let granted = rd_core::granted_scopes(scopes.iter().map(String::as_str));
            if !COMPAT_SCOPES.iter().all(|scope| granted.contains(scope)) {
                return CompatAccess::Refused;
            }
            note_token_use(state, digest);
            note_token_audit(state, id, label, scopes);
            CompatAccess::Granted
        }
        Ok(None) => CompatAccess::Refused,
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                "could not read the token store for a compatibility client"
            );
            CompatAccess::Unavailable
        }
    }
}

/// The transport gate on `/mcp`: any credential carrying at least one API scope gets in.
///
/// It used to demand `api:*` specifically, which made the endpoint all-or-nothing — an
/// assistant that should only watch the queue had to be handed a token that could also read
/// every stored account. The per-tool policy in [`crate::mcp`] is what decides now, so this
/// only has to establish that there *is* a credential; refusing a narrower one here would put
/// the decision back in a place that cannot see which tool was called.
///
/// The credential it resolved travels on with the request as [`Granted`] and the
/// [`crate::audit::Actor`], the way `require_session` hands them to a handler: the MCP server
/// reads them from the request parts instead of looking the token up twice more per tool call
/// (audit 1.9.1, API-14).
pub async fn require_api_token(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let from_this_machine =
        crate::client::from_this_machine(request.extensions(), request.headers());
    let host = request_host(request.uri(), request.headers());
    // Every method, not only the unsafe ones (RD-1190-22): the MCP specification asks a server
    // to check `Origin` on every connection, the `GET` event stream included.
    super::middleware::refuse_foreign(&state, host.as_deref(), request.headers(), false).await?;
    // At least one *API* scope, not merely a non-empty set: a browser-capture token carries
    // `capture:*` and must not reach this endpoint through a check that only counts. With the
    // login switched off for this machine the credential is every scope, as before.
    let (granted, actor) = credential(&state, request.headers(), from_this_machine).await;
    // `api:metrics` is an API scope for the token editor's purposes, but it opens nothing
    // here: a scrape token must not be able to open an MCP session either (RD-110-01).
    if !granted
        .iter()
        .any(|scope| rd_core::Scope::API.contains(scope) && *scope != rd_core::Scope::Metrics)
    {
        // Said out loud, because this refusal is otherwise completely silent. An MCP client
        // shows a failed connection and nothing else, the service logs nothing at all, and
        // the administrator is left comparing a token they cannot read against a store that
        // only holds its digest. One line naming the *shape* of the credential turns that
        // into a reading.
        //
        // Resolved before the macro rather than inside it: awaiting in a `tracing` field
        // expression holds the macro's own internals across the await point, which makes the
        // whole middleware future `!Send` and fails the build with an unrelated-looking
        // `Service` trait error at the router.
        let reason = refusal_reason(&state, request.headers()).await;
        tracing::warn!(
            path = %request.uri().path(),
            reason = %reason,
            "an API token was refused"
        );
        return Ok(unauthorized_with_challenge());
    }
    request.extensions_mut().insert(Granted(granted));
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

/// The `401` a missing or unusable API token earns, carrying the challenge the MCP
/// specification requires.
///
/// A bare `401` tells a client that something is wrong but not what kind of credential would
/// be right, and MCP clients read `WWW-Authenticate` to find out. No `resource_metadata`
/// parameter is offered: this service issues bearer tokens itself and runs no authorization
/// server, so pointing at one would send an OAuth-capable client into a flow that cannot end.
pub(super) fn unauthorized_with_challenge() -> Response {
    use axum::response::IntoResponse;

    let mut response =
        ApiError::unauthorized("api.token_required", "A valid API token is required")
            .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer realm=\"rDownloader\""),
    );
    response
}

/// The bearer credential on this request, or why it is unusable before the store is consulted.
///
/// Split out from [`refusal_reason`] because these are the cases that need no database and are
/// the ones that actually bite: every one of them reaches the service as a plain `401` that is
/// indistinguishable from a wrong token, and a client configured through a dialog can produce
/// any of them without showing the user anything.
pub(super) fn malformed_credential(headers: &HeaderMap) -> Result<&str, String> {
    let sent = headers.get_all(header::AUTHORIZATION).iter().count();
    if sent == 0 {
        return Err("no Authorization header was sent".to_owned());
    }
    if sent > 1 {
        // Only the first is ever read. A client configured with two sources for the same
        // header — a static one and one from an environment variable, say — therefore
        // authenticates as whichever happens to come first, which is invisible from both ends.
        return Err(format!(
            "{sent} Authorization headers were sent, and only the first is read"
        ));
    }
    let Some(token) = bearer_token(headers) else {
        return Err("the Authorization header is not a Bearer credential".to_owned());
    };
    if token.is_empty() {
        // What an environment variable that did not resolve looks like on the wire.
        return Err("the Bearer credential is empty".to_owned());
    }
    Ok(token)
}

/// Why the credential on this request was not accepted, in the words a log line needs.
///
/// The token never appears. What appears is the first eight characters of its SHA-256 digest —
/// the same value `capture_tokens.token_sha256` stores — so an administrator can match the
/// line against the token list while the secret stays with the client.
pub(super) async fn refusal_reason(state: &AppState, headers: &HeaderMap) -> String {
    let token = match malformed_credential(headers) {
        Err(reason) => return reason,
        Ok(token) => token,
    };
    let digest = digest_of(token);
    let fingerprint = &digest[..8];
    match state.database.capture_token_scopes(&digest).await {
        Ok(Some(scopes)) => format!(
            "the token with digest {fingerprint} holds [{}], which contains no api scope",
            scopes.join(", ")
        ),
        Ok(None) => {
            format!("no active token has digest {fingerprint}; it is unknown here or revoked")
        }
        Err(error) => {
            format!("the token with digest {fingerprint} could not be looked up: {error}")
        }
    }
}

/// The capture surface's guard: a live token holding `capture:*`, whatever else it holds.
///
/// The scopes it resolved travel on with the request as [`Granted`], the way `require_session`
/// hands them to a handler: the summary reads from them whether the agent may control the queue,
/// and [`require_capture_queue`] whether this request may (RD-1100-06).
pub async fn require_capture(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Owned before anything is awaited; see `RequestFacts`.
    let digest = bearer_token(request.headers()).map(digest_of);
    // The identity, not only the scopes: a handler that audits what the agent changed names the
    // token that did it (audit 2026-10-08, API-02), as `require_api_token` hands it on.
    let identity = match digest {
        Some(digest) => match state.database.capture_token_identity(&digest).await {
            Ok(Some((id, label, scopes)))
                if rd_core::scopes_satisfy(
                    scopes.iter().map(String::as_str),
                    rd_core::CAPTURE_SCOPE,
                ) =>
            {
                note_token_use(&state, digest);
                Some((crate::audit::Actor::token(id.to_string(), label), scopes))
            }
            Ok(_) => None,
            // A token nobody can look up grants nothing, as before.
            Err(error) => {
                tracing::warn!(error = %error, "could not read a capture token's scopes");
                None
            }
        },
        None => None,
    };
    let Some((actor, scopes)) = identity else {
        return Err(ApiError::unauthorized(
            "capture.token_required",
            "A valid capture token is required",
        ));
    };
    request
        .extensions_mut()
        .insert(Granted(rd_core::granted_scopes(
            scopes.iter().map(String::as_str),
        )));
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

/// The tray's queue control (RD-1100-06): a capture token that was paired with `capture:queue`.
///
/// Runs inside [`require_capture`], which established the token and left its scopes on the
/// request. A capture token without the right is refused with the `403` the scope policy gives
/// every other route a credential does not cover, and the scope it lacks, so the agent can tell
/// "not allowed" from "not paired".
pub async fn require_capture_queue(request: Request, next: Next) -> Result<Response, ApiError> {
    let holds = request
        .extensions()
        .get::<Granted>()
        .is_some_and(|granted| granted.holds(rd_core::Scope::CaptureQueue));
    if !holds {
        return Err(ApiError::forbidden(
            "auth.scope_insufficient",
            "This token does not hold the scope this route requires",
        )
        .with_param("scope", rd_core::CAPTURE_QUEUE_SCOPE));
    }
    Ok(next.run(request).await)
}
