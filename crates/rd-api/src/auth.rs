use std::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Instant,
};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{ApiError, AppState, scope_policy};

const PASSWORD_SETTING: &str = "auth.admin_password_hash";

/// The scopes a request carries, for handlers that narrow their own output.
///
/// Replaces the former `AccessLevel` two-value enum, which could only say "everything" or
/// "the read allowlist". The event stream needs to know *which* areas a subscriber may see,
/// not how privileged it feels.
#[derive(Clone, Debug, Default)]
pub(crate) struct Granted(pub(crate) Vec<rd_core::Scope>);

impl Granted {
    /// Whether this credential carries `scope`.
    pub(crate) fn holds(&self, scope: rd_core::Scope) -> bool {
        self.0.contains(&scope)
    }

    /// Whether an event of this kind may be streamed to this subscriber.
    pub(crate) fn may_observe(&self, kind: &rd_core::EventKind) -> bool {
        self.0.contains(&rd_core::Scope::of_event(kind))
    }
}

const SESSION_COOKIE: &str = "rd_session";

/// The session limits in force, readable on every request without a lock (RD-130-09).
///
/// Two atomics rather than one value behind a lock: a request reads both on its way through
/// the middleware, and a settings save that lands between the two reads costs one request
/// the old value of one limit, which the next request corrects.
struct LiveLimits {
    idle_hours: AtomicU32,
    max_hours: AtomicU32,
}

impl Default for LiveLimits {
    fn default() -> Self {
        let limits = rd_core::SessionLimits::default();
        Self {
            idle_hours: AtomicU32::new(limits.idle_hours),
            max_hours: AtomicU32::new(limits.max_hours),
        }
    }
}

/// Session registry over the database, plus the login limiter.
///
/// Sessions used to be a `HashMap<String, Instant>` in this struct. That could not survive a
/// restart, could not be listed, could not be revoked, and stored the bearer itself as the
/// map key. All four are now the database's problem, and the bearer is stored only as a
/// SHA-256 digest.
///
/// The limiter stays in memory on purpose: persisting it would mean a disk write per failed
/// login, which is an amplification the attacker controls, to buy a guarantee that only
/// matters against someone who can also restart the service. See `rd_authn::throttle`.
#[derive(Clone, Default)]
pub struct AuthService {
    /// Mirrors `SettingsResponse::admin_login_disabled` for request handling.
    disabled: std::sync::Arc<AtomicBool>,
    /// Mirrors `SettingsResponse::session_idle_hours` and `session_max_hours`.
    limits: std::sync::Arc<LiveLimits>,
    throttle: std::sync::Arc<RwLock<Option<rd_authn::LoginThrottle>>>,
}

impl AuthService {
    /// Whether the administrator login is switched off.
    #[must_use]
    pub fn disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Switches the administrator login on or off for subsequent requests.
    pub fn set_disabled(&self, disabled: bool) {
        self.disabled.store(disabled, Ordering::Relaxed);
    }

    /// How long a session lasts, as the settings say now.
    #[must_use]
    pub fn session_limits(&self) -> rd_core::SessionLimits {
        rd_core::SessionLimits {
            idle_hours: self.limits.idle_hours.load(Ordering::Relaxed),
            max_hours: self.limits.max_hours.load(Ordering::Relaxed),
        }
    }

    /// Applies new session limits to every following request, existing sessions included.
    pub fn set_session_limits(&self, limits: rd_core::SessionLimits) {
        self.limits
            .idle_hours
            .store(limits.idle_hours, Ordering::Relaxed);
        self.limits
            .max_hours
            .store(limits.max_hours, Ordering::Relaxed);
    }

    /// Loads the persisted login switch and session limits from the stored service settings.
    ///
    /// An unset switch means "login enabled", and so does one stored with the wrong type —
    /// but the second case is now reported rather than read as a deliberate choice, because a
    /// switch that silently reads as "off" is an authentication decision nobody made.
    pub async fn load(&self, state: &AppState) -> anyhow::Result<()> {
        let disabled = state
            .database
            .service_setting_field::<bool>("admin_login_disabled")
            .await?
            .unwrap_or(false);
        self.set_disabled(disabled);
        self.set_session_limits(state.database.session_limits().await?);
        Ok(())
    }

    /// Returns whether a password was configured.
    pub async fn is_configured(&self, state: &AppState) -> Result<bool, ApiError> {
        Ok(state
            .database
            .get_setting(PASSWORD_SETTING)
            .await?
            .is_some())
    }

    /// Stores the first administrator password.
    pub async fn setup(&self, state: &AppState, password: &str) -> Result<(), ApiError> {
        if self.is_configured(state).await? {
            return Err(ApiError::conflict(
                "auth.setup_completed",
                "Setup has already been completed",
            ));
        }
        self.store_password(state, password).await
    }

    /// Writes `password` as the administrator password, whatever is there now.
    ///
    /// The one place that hashes, shared by [`Self::setup`] and the password change
    /// (RD-120-22) so the two cannot drift apart on the policy or on the hash parameters.
    /// It deliberately checks *no* credential: who may call it is the caller's question, and
    /// a function that both authorised and wrote would let the two disagree.
    pub async fn store_password(&self, state: &AppState, password: &str) -> Result<(), ApiError> {
        validate_password(password)?;
        let hash = hash_password(password)?;
        state
            .database
            .set_setting(PASSWORD_SETTING.to_owned(), serde_json::Value::String(hash))
            .await?;
        Ok(())
    }

    /// What the limiter says about an attempt from `client`.
    pub async fn throttle_check(&self, client: std::net::IpAddr) -> rd_authn::Decision {
        let mut guard = self.throttle.write().await;
        guard
            .get_or_insert_with(|| {
                rd_authn::LoginThrottle::new(rd_authn::ThrottleSettings::default())
            })
            .check(client, Instant::now())
    }

    async fn record_login_failure(&self, client: std::net::IpAddr) {
        let mut guard = self.throttle.write().await;
        guard
            .get_or_insert_with(|| {
                rd_authn::LoginThrottle::new(rd_authn::ThrottleSettings::default())
            })
            .record_failure(client, Instant::now());
    }

    async fn record_login_success(&self, client: std::net::IpAddr) {
        let mut guard = self.throttle.write().await;
        guard
            .get_or_insert_with(|| {
                rd_authn::LoginThrottle::new(rd_authn::ThrottleSettings::default())
            })
            .record_success(client, Instant::now());
    }

    /// Records a failed sign-in against the limiter.
    ///
    /// Split out from the password check because the login now has two credentials to weigh
    /// and only decides at the end which way it went; the limiter has to be told once, there.
    pub async fn note_failed_login(&self, client: std::net::IpAddr) {
        self.record_login_failure(client).await;
    }

    /// Opens a session for a caller whose credentials have already been accepted.
    ///
    /// Deliberately takes no password: the checks live in the handler, which is the only place
    /// that knows whether a second factor was also required. A function that both verified and
    /// issued would have to be told about the second factor too, and the two would then be able
    /// to disagree about what "authenticated" means.
    pub async fn open_session(
        &self,
        state: &AppState,
        client: std::net::IpAddr,
        user_agent: Option<String>,
    ) -> Result<OpenedSession, ApiError> {
        self.record_login_success(client).await;
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let id = rd_core::SessionId::new();
        // The row's own expiry is the maximum in force at sign-in; the limits are applied
        // again on every read, which is what lets a shorter setting reach this session later.
        let max_hours = self.session_limits().max_hours;
        state
            .database
            .create_session(
                id,
                digest_of(&token),
                user_agent,
                Some(client.to_string()),
                i64::from(max_hours),
            )
            .await?;
        Ok(OpenedSession {
            token,
            id,
            max_age_seconds: u64::from(max_hours) * 60 * 60,
        })
    }

    /// Whether `password` is the administrator password.
    ///
    /// Separate from [`Self::login`] because two callers need the check without a session
    /// coming out of it: switching the second factor off, and any later step-up prompt. It
    /// deliberately does not touch the login limiter — those callers are already authenticated,
    /// so a mistyped password there is not a brute-force attempt on the front door.
    pub async fn password_matches(
        &self,
        state: &AppState,
        password: &str,
    ) -> Result<bool, ApiError> {
        let Some(value) = state.database.get_setting(PASSWORD_SETTING).await? else {
            return Ok(false);
        };
        Ok(value
            .as_str()
            .and_then(|encoded| PasswordHash::new(encoded).ok())
            .is_some_and(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            }))
    }

    /// Ends the session behind this request's credential, if it has one.
    pub async fn logout(&self, state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
        let Some(token) = session_token(headers) else {
            return Ok(());
        };
        let digest = digest_of(token);
        if let Some(session) = state
            .database
            .session_for_digest(&digest, self.session_limits())
            .await?
        {
            state.database.revoke_session(session.id).await?;
        }
        Ok(())
    }

    /// Checks a cookie or bearer token against the stored sessions and the limits in force.
    ///
    /// Reads from the reader pool; the one write on this path — advancing `last_used_at` — is
    /// skipped unless the stored value is already a minute old, so an authenticated request
    /// does not queue a serialized write.
    pub async fn authenticated(&self, state: &AppState, headers: &HeaderMap) -> bool {
        self.current_session(state, headers).await.is_some()
    }

    /// The session behind this request, if it has a live one.
    pub async fn current_session(
        &self,
        state: &AppState,
        headers: &HeaderMap,
    ) -> Option<rd_core::Session> {
        let token = session_token(headers)?;
        let digest = digest_of(token);
        let session = match state
            .database
            .session_for_digest(&digest, self.session_limits())
            .await
        {
            Ok(session) => session?,
            Err(error) => {
                tracing::warn!(error = %error, "could not read the session store");
                return None;
            }
        };
        let stale = chrono::Utc::now() - session.last_used_at
            > chrono::Duration::seconds(rd_db::SESSION_TOUCH_INTERVAL_SECONDS);
        if stale && let Err(error) = state.database.touch_session(digest).await {
            // Losing the timestamp costs accuracy in the inventory and nothing else, so it
            // must not cost the request its authentication.
            tracing::warn!(error = %error, "could not record session use");
        }
        Some(session)
    }

    /// Checks only revocable `capture:*` bearer tokens from persistent storage.
    pub async fn capture_authenticated(&self, state: &AppState, headers: &HeaderMap) -> bool {
        scoped_token_authenticated(state, headers, rd_core::CAPTURE_SCOPE).await
    }

    /// The `Set-Cookie` value that clears the session cookie.
    ///
    /// Deliberately without `Secure`: clearing a cookie has to work whatever the current
    /// configuration says, including after the configuration changed under it.
    pub const EXPIRED_COOKIE: &'static str =
        "rd_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";

    /// Creates the `Set-Cookie` value for a session token, kept by the browser for
    /// `max_age_seconds` — the maximum lifetime the session was opened under.
    ///
    /// `secure` comes from the proxy contract rather than from the request, because the
    /// request cannot tell: a TLS-terminating proxy forwards plain HTTP, so the connection
    /// this service sees is never the connection the browser made. Getting it wrong in either
    /// direction fails silently — a missing `Secure` sends the cookie over plain HTTP, and a
    /// spurious one makes the browser drop it, so signing in appears to work and the next
    /// request is unauthenticated.
    #[must_use]
    pub fn cookie(token: &str, secure: bool, base_path: &str, max_age_seconds: u64) -> String {
        let path = if base_path.is_empty() { "/" } else { base_path };
        let secure = if secure { "; Secure" } else { "" };
        format!(
            "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path={path}{secure}; \
             Max-Age={max_age_seconds}"
        )
    }
}

/// An owned snapshot of what a request is, taken before anything is awaited.
///
/// `axum::body::Body` is not `Sync`, so a `&Request` alive across an `.await` makes this
/// middleware's future non-`Send` and the layer stops being a `Service` at all.
#[derive(Clone)]
struct RequestFacts {
    path: String,
    method: Method,
    headers: HeaderMap,
}

impl RequestFacts {
    /// `None` when no route matched: the 404 fallback is not a policy question.
    fn of(request: &Request) -> Option<Self> {
        Some(Self {
            path: request
                .extensions()
                .get::<axum::extract::MatchedPath>()?
                .as_str()
                .to_owned(),
            method: request.method().clone(),
            headers: request.headers().clone(),
        })
    }
}

/// The scopes this request actually carries.
///
/// A session, or a service with the login switched off, holds every API scope — that is what
/// an interactive administrator is. A bearer holds what was minted into it, expanded through
/// the implication edges, which is where a legacy `api:*` becomes the full set.
pub(crate) async fn granted_scopes(state: &AppState, headers: &HeaderMap) -> Vec<rd_core::Scope> {
    credential(state, headers).await.0
}

/// The scopes this request carries **and who is carrying them**.
///
/// One lookup answers both, which is the point: the audit log names an actor (RD-110-03), and
/// asking the database a second time to find out who it was would let the two answers
/// disagree about the same request. The token's bearer value is never read here — only its
/// digest is, and only to find the row; the id and the label that come back are the two
/// things a person already sees in the token list.
pub(crate) async fn credential(
    state: &AppState,
    headers: &HeaderMap,
) -> (Vec<rd_core::Scope>, crate::audit::Actor) {
    let full = rd_core::Scope::API.to_vec();
    if state.auth.disabled() {
        // Nobody signed in, and every caller is an administrator. "Anonymous" is the honest
        // word for that, and it is worth being able to filter an audit log by it.
        return (full, crate::audit::Actor::anonymous());
    }
    if let Some(session) = state.auth.current_session(state, headers).await {
        return (full, crate::audit::Actor::session(session.id.to_string()));
    }
    let Some(token) = bearer_token(headers) else {
        return (Vec::new(), crate::audit::Actor::anonymous());
    };
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    match state.database.capture_token_identity(&digest).await {
        Ok(Some((id, label, scopes))) => {
            note_token_use(state, digest.clone());
            note_token_audit(state, id, label.clone(), scopes.clone());
            (
                rd_core::granted_scopes(scopes.iter().map(String::as_str)),
                crate::audit::Actor::token(id.to_string(), label),
            )
        }
        // A token nobody can look up grants nothing; a database error must not grant more
        // than a missing token does.
        Ok(None) => (Vec::new(), crate::audit::Actor::anonymous()),
        Err(error) => {
            tracing::warn!(error = %error, "could not read token scopes for the policy check");
            (Vec::new(), crate::audit::Actor::anonymous())
        }
    }
}

/// How rarely one token's use is written to the audit log, keyed by token id.
///
/// A scrape target hits this service every fifteen seconds forever. One audit record per
/// request would bury every other record within a day and turn retention into a rolling
/// window of one client's polling; one per token per hour is what "this token is in use"
/// actually needs to say. In memory on purpose — after a restart the first use of every token
/// is recorded again, which is information rather than noise.
static TOKEN_USE_SEEN: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<rd_core::CaptureTokenId, chrono::DateTime<chrono::Utc>>,
    >,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Records that a token was accepted, at most once per interval.
///
/// Detached, unlike every other audit write: this is not an action somebody asked for, it is
/// an observation about a request that is already being served, and making that request wait
/// for a database write would put the audit log in the path of every machine client.
fn note_token_audit(
    state: &AppState,
    id: rd_core::CaptureTokenId,
    label: String,
    scopes: Vec<String>,
) {
    let now = chrono::Utc::now();
    {
        let Ok(mut seen) = TOKEN_USE_SEEN.lock() else {
            return;
        };
        if let Some(last) = seen.get(&id)
            && (now - *last).num_seconds() < rd_core::AUDIT_TOKEN_USE_INTERVAL_SECONDS
        {
            return;
        }
        seen.insert(id, now);
    }
    let state = state.clone();
    tokio::spawn(async move {
        crate::audit::record(
            &state,
            crate::audit::AuditEvent::success(rd_core::AuditAction::TokenUsed)
                .actor(crate::audit::Actor::token(id.to_string(), label))
                .target("token", id)
                .detail("scopes", scopes.join(" ")),
        )
        .await;
    });
}

/// One decision, taken from the scope policy table.
///
/// The shape it replaces was a ladder of four credential checks, each with its own idea of
/// what the credential could reach — a session and an `api:*` token were waved through
/// wholesale, and `api:read` was narrowed by a hardcoded route allowlist that lived in this
/// file. That could only express two privilege levels, and every new route silently joined
/// the "session only" one.
///
/// Now the credential answers one question — which scopes does it carry — and the table
/// answers the other — which scope does this route cost. The two are compared once.
pub(crate) async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Captured before anything is awaited; see [`RequestFacts`].
    let facts = RequestFacts::of(&request);
    let (granted, actor) = match facts.as_ref() {
        Some(facts) => credential(&state, &facts.headers).await,
        None => (Vec::new(), crate::audit::Actor::anonymous()),
    };

    if granted.is_empty() {
        // No credential at all. The setup gate comes *after* the token check on purpose: a
        // revocable bearer is a credential in its own right, and a machine client must not
        // start failing because the administrator password of an unattended service was
        // never set.
        if !state.auth.is_configured(&state).await? {
            return Err(ApiError::unauthorized(
                "auth.setup_pending",
                "Setup has not been completed yet",
            ));
        }
        return Err(ApiError::unauthorized(
            "auth.session_required",
            "Login required",
        ));
    }

    if let Some(refusal) = scope_refusal(facts.as_ref(), &granted) {
        return Err(refusal);
    }
    request.extensions_mut().insert(Granted(granted));
    // Who acted, for any handler that writes an audit record (RD-110-03). Established here
    // because this is the one place every authenticated request passes through.
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

/// The refusal this route's requirement produces for these scopes, if any.
fn scope_refusal(facts: Option<&RequestFacts>, granted: &[rd_core::Scope]) -> Option<ApiError> {
    let facts = facts?;
    let Some(requirement) = scope_policy::requirement(&facts.path, &facts.method) else {
        // Cannot happen: the exhaustiveness test in `scope_policy` forbids a route with no
        // entry. "Cannot happen" is not a thing to base an authorisation decision on, so
        // this fails closed, loudly, rather than falling through to allow.
        tracing::warn!(
            path = %facts.path,
            method = %facts.method,
            "no scope decision exists for this route; refusing"
        );
        return Some(ApiError::forbidden(
            "scope.route_unclassified",
            "This route has no scope decision",
        ));
    };
    let required = match requirement {
        scope_policy::Requirement::Public => return None,
        scope_policy::Requirement::Scope(scope) => scope,
    };
    if granted.contains(&required) {
        return None;
    }
    // A 403 rather than a 401: the credential was accepted and simply does not cover this.
    // A 401 would send a dashboard back to a login screen it cannot use.
    Some(
        ApiError::forbidden(
            "auth.scope_insufficient",
            "This token does not hold the scope this route requires",
        )
        .with_param("scope", required.as_str()),
    )
}

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
fn digest_of(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The stored digest of this request's session credential, if it carries one.
pub(crate) fn session_digest(headers: &HeaderMap) -> Option<String> {
    session_token(headers).map(digest_of)
}

/// The session credential, from the bearer header or the cookie.
///
/// Bearer wins, as it did before: a command-line client that sets both should get the one it
/// chose deliberately rather than a cookie a browser left behind.
fn session_token(headers: &HeaderMap) -> Option<&str> {
    bearer_token(headers).or_else(|| cookie_token(headers))
}

async fn scoped_token_authenticated(state: &AppState, headers: &HeaderMap, scope: &str) -> bool {
    let Some(token) = bearer_token(headers) else {
        return false;
    };
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    let valid = state
        .database
        .capture_token_valid(&digest, scope)
        .await
        .unwrap_or(false);
    if valid {
        note_token_use(state, digest);
    }
    valid
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
/// must not queue behind it for a field nobody reads in real time; the throttle inside the
/// statement is what keeps a busy client from filing one write per request.
fn note_token_use(state: &AppState, digest: String) {
    let database = state.database.clone();
    tokio::spawn(async move {
        if let Err(error) = database.touch_capture_token(digest).await {
            tracing::debug!(error = %error, "a machine token's last use was not recorded");
        }
    });
}

/// The transport gate on `/mcp`: any credential carrying at least one API scope gets in.
///
/// It used to demand `api:*` specifically, which made the endpoint all-or-nothing — an
/// assistant that should only watch the queue had to be handed a token that could also read
/// every stored account. The per-tool policy in [`crate::mcp`] is what decides now, so this
/// only has to establish that there *is* a credential; refusing a narrower one here would put
/// the decision back in a place that cannot see which tool was called.
pub(crate) async fn require_api_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if state.auth.disabled() {
        return Ok(next.run(request).await);
    }
    // At least one *API* scope, not merely a non-empty set: a browser-capture token carries
    // `capture:*` and must not reach this endpoint through a check that only counts.
    let granted = granted_scopes(&state, request.headers()).await;
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
    Ok(next.run(request).await)
}

/// The `401` a missing or unusable API token earns, carrying the challenge the MCP
/// specification requires.
///
/// A bare `401` tells a client that something is wrong but not what kind of credential would
/// be right, and MCP clients read `WWW-Authenticate` to find out. No `resource_metadata`
/// parameter is offered: this service issues bearer tokens itself and runs no authorization
/// server, so pointing at one would send an OAuth-capable client into a flow that cannot end.
fn unauthorized_with_challenge() -> Response {
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
fn malformed_credential(headers: &HeaderMap) -> Result<&str, String> {
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
async fn refusal_reason(state: &AppState, headers: &HeaderMap) -> String {
    let token = match malformed_credential(headers) {
        Err(reason) => return reason,
        Ok(token) => token,
    };
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
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

pub(crate) async fn require_capture(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !state
        .auth
        .capture_authenticated(&state, request.headers())
        .await
    {
        return Err(ApiError::unauthorized(
            "capture.token_required",
            "A valid capture token is required",
        ));
    }
    Ok(next.run(request).await)
}

/// Argon2id over a fresh 16-byte salt, in the PHC string form the setting stores.
fn hash_password(password: &str) -> Result<String, ApiError> {
    let mut salt_bytes = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|error| {
        tracing::error!(%error, "failed to encode password salt");
        ApiError::bad_request("auth.password_hash_failed", "Password could not be hashed")
    })?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| {
            tracing::error!(%error, "failed to hash password");
            ApiError::bad_request("auth.password_hash_failed", "Password could not be hashed")
        })?
        .to_string())
}

/// The password policy. One function, so the first password and every later one are judged
/// by the same rule (RD-120-22).
pub(crate) fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 10 {
        return Err(ApiError::bad_request(
            "auth.password_too_short",
            "The password must be at least 10 characters long",
        )
        .with_param("min", 10));
    }
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE}=")))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, Method};
    use rd_core::{EventKind, Scope};

    use super::{Granted, malformed_credential};

    fn read_only() -> Granted {
        Granted(vec![Scope::Read])
    }

    fn session() -> Granted {
        Granted(Scope::API.to_vec())
    }

    /// The four ways a credential arrives unusable, each with its own sentence.
    ///
    /// All four reach the service as the same `401`, so the log line is the only thing that
    /// separates them. The MCP endpoint is where this matters: a connector dialog can be
    /// filled in wrongly in every one of these ways and reports nothing but "failed".
    #[test]
    fn each_shape_of_unusable_credential_is_named() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            malformed_credential(&headers).expect_err("no header is unusable"),
            "no Authorization header was sent"
        );

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic abc".parse().expect("value"),
        );
        assert_eq!(
            malformed_credential(&headers).expect_err("Basic is unusable"),
            "the Authorization header is not a Bearer credential"
        );

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer ".parse().expect("value"),
        );
        assert_eq!(
            malformed_credential(&headers).expect_err("an empty Bearer is unusable"),
            "the Bearer credential is empty"
        );

        // The trap that costs the most time: two sources configured for one header. The first
        // wins, so a correct token in the second is never read and the refusal looks like a
        // wrong token rather than a duplicate.
        headers.append(
            axum::http::header::AUTHORIZATION,
            "Bearer real".parse().expect("value"),
        );
        assert_eq!(
            malformed_credential(&headers).expect_err("a duplicate header is unusable"),
            "2 Authorization headers were sent, and only the first is read"
        );
    }

    #[test]
    fn a_single_well_formed_bearer_is_handed_on_for_the_store_to_judge() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer s3cret".parse().expect("value"),
        );
        assert_eq!(malformed_credential(&headers).expect("a token"), "s3cret");
    }

    #[test]
    fn a_read_only_subscriber_sees_queue_events_only() {
        for kind in [
            EventKind::DownloadProgress,
            EventKind::DownloadState,
            EventKind::PackageState,
            EventKind::PostprocessProgress,
            EventKind::StorageCapacity,
            EventKind::TorrentStats,
        ] {
            assert!(
                read_only().may_observe(&kind),
                "{kind:?} is what a dashboard exists to show"
            );
        }
    }

    #[test]
    fn a_read_only_subscriber_never_sees_configuration_events() {
        // These carry the ids of accounts, proxies, credentials, plugins and settings
        // changes. Streaming them would hand a monitoring token a map of the installation.
        for kind in [
            EventKind::AccountChanged,
            EventKind::AuthProfileChanged,
            EventKind::AutomationChanged,
            EventKind::CaptureChanged,
            EventKind::CategoryChanged,
            EventKind::CollectorChanged,
            EventKind::CollectorIntake,
            EventKind::HotFolderChanged,
            EventKind::ManagedToolChanged,
            EventKind::PluginCatalogChanged,
            EventKind::PluginChanged,
            EventKind::PostprocessCatalogChanged,
            EventKind::PluginTrustChanged,
            EventKind::ProxyChanged,
            EventKind::RemoteCredentialChanged,
            EventKind::System,
            EventKind::UsenetChanged,
        ] {
            assert!(
                !read_only().may_observe(&kind),
                "{kind:?} reached a read-only subscriber"
            );
        }
    }

    #[test]
    fn a_session_sees_the_whole_bus() {
        for kind in [
            EventKind::AccountChanged,
            EventKind::DownloadState,
            EventKind::PluginChanged,
            EventKind::CollectorIntake,
        ] {
            assert!(session().may_observe(&kind), "{kind:?}");
        }
    }

    /// A subscriber with no scopes at all sees nothing, rather than everything.
    ///
    /// This is the shape of the mistake worth guarding: the previous default, applied when
    /// the extension was absent, was "full access". Mounting the stream outside the session
    /// layer would then have published the whole bus to anyone who asked.
    #[test]
    fn a_subscriber_with_no_scopes_sees_nothing() {
        let none = Granted::default();
        for kind in [
            EventKind::DownloadProgress,
            EventKind::AccountChanged,
            EventKind::System,
        ] {
            assert!(!none.may_observe(&kind), "{kind:?} reached an empty grant");
        }
    }

    /// A credential scope must not double as a way to watch the queue.
    #[test]
    fn a_secrets_only_subscriber_sees_only_credential_events() {
        let secrets = Granted(vec![Scope::Secrets]);
        assert!(secrets.may_observe(&EventKind::AccountChanged));
        assert!(!secrets.may_observe(&EventKind::DownloadProgress));
    }

    /// An event's scope follows the scope of the write that produces it.
    ///
    /// The trust store's four writes -- trusting and revoking a signing key, withdrawing and
    /// reinstating a package digest -- sit behind `Secrets`-scoped routes, while installing,
    /// enabling, disabling and removing a plugin is `Admin`. Both halves used to announce
    /// themselves as `PluginChanged`, which was wrong in both directions at once: the token
    /// allowed to make the trust write never saw the event it caused, and the key ids and
    /// digests those payloads carry were delivered to `Admin` subscribers, who may not read
    /// the tables they name.
    ///
    /// Both directions are asserted because [`Granted::may_observe`] checks exact possession
    /// and not [`Scope::satisfies`]: `Admin` and `Secrets` are siblings and neither confers
    /// the other, so a kind filed under the wrong one is invisible to exactly the scope that
    /// should see it. Nothing else in the suite would notice.
    #[test]
    fn plugin_trust_is_secrets_and_plugin_administration_is_admin() {
        let secrets = Granted(vec![Scope::Secrets]);
        let admin = Granted(vec![Scope::Admin]);

        assert!(
            secrets.may_observe(&EventKind::PluginTrustChanged),
            "the scope that may write a trust decision must see the event it caused"
        );
        assert!(
            !admin.may_observe(&EventKind::PluginTrustChanged),
            "a key id or a digest was streamed to a scope that may not read those tables"
        );

        assert!(
            admin.may_observe(&EventKind::PluginChanged),
            "installing or removing a plugin is administration and must reach it"
        );
        assert!(
            !secrets.may_observe(&EventKind::PluginChanged),
            "the credential scope must not double as a view of the plugin inventory"
        );
    }

    fn facts(path: &str, method: Method) -> super::RequestFacts {
        super::RequestFacts {
            path: path.to_owned(),
            method,
            headers: HeaderMap::new(),
        }
    }

    /// The positive direction of the policy, decided rather than executed.
    ///
    /// `tests/scope_matrix.rs` drives the *negative* direction through the real router across
    /// every route. It deliberately does not drive the positive one: letting 233 requests
    /// through reaches real handlers, some of which open network connections or spawn external
    /// tools. So the "a token that holds the scope gets through" half is checked here, against
    /// the same table, where it costs nothing.
    #[test]
    fn a_scope_the_route_requires_is_not_refused() {
        for (path, method, required) in crate::policy_rows() {
            let Some(required) = required else { continue };
            let Some(scope) = Scope::parse(required) else {
                panic!("{required} is not a scope");
            };
            let method = Method::from_bytes(method.as_bytes()).expect("method");
            assert!(
                super::scope_refusal(Some(&facts(path, method)), &[scope]).is_none(),
                "{path} requires {required} and refused a token holding exactly it"
            );
        }
    }

    /// A public route is reachable with no scope at all.
    #[test]
    fn a_public_route_needs_nothing() {
        assert!(super::scope_refusal(Some(&facts("/api/v1/health", Method::GET)), &[]).is_none());
    }

    /// A route the table does not know is refused, not waved through.
    ///
    /// The exhaustiveness test in `scope_policy` means this cannot happen. It is still the
    /// branch worth pinning: "cannot happen" and "fails open when it does" is how
    /// authorisation holes are made.
    #[test]
    fn an_unclassified_route_fails_closed() {
        let refusal =
            super::scope_refusal(Some(&facts("/api/v1/invented", Method::GET)), Scope::API);
        assert_eq!(
            refusal.map(|error| error.code().to_owned()),
            Some("scope.route_unclassified".to_owned())
        );
    }

    /// A request that matched no route is not a policy question, and must not be refused as one.
    #[test]
    fn an_unmatched_request_is_left_to_the_fallback() {
        assert!(super::scope_refusal(None, &[]).is_none());
    }

    /// The refusal names the scope that was missing, so the message can say what to do.
    #[test]
    fn a_refusal_names_the_scope_it_wanted() {
        let refusal = super::scope_refusal(
            Some(&facts("/api/v1/accounts", Method::GET)),
            &[Scope::Read],
        )
        .expect("a read-only token must not reach accounts");
        assert_eq!(refusal.code(), "auth.scope_insufficient");
    }
}
