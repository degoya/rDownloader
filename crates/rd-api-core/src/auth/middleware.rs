//! The request guards: what a request carries, whether it may reach a route, and the refusals.

use rd_core::AuditChannel;

use super::*;

/// An owned snapshot of what a request is, taken before anything is awaited.
///
/// `axum::body::Body` is not `Sync`, so a `&Request` alive across an `.await` makes this
/// middleware's future non-`Send` and the layer stops being a `Service` at all.
#[derive(Clone)]
pub(super) struct RequestFacts {
    pub(super) path: String,
    pub(super) method: Method,
    pub(super) headers: HeaderMap,
    /// [`request_host`] of the request.
    pub(super) host: Option<String>,
    pub(super) from_this_machine: bool,
}

impl RequestFacts {
    /// `None` when no route matched: the 404 fallback is not a policy question.
    pub(super) fn of(request: &Request) -> Option<Self> {
        Some(Self {
            path: request
                .extensions()
                .get::<axum::extract::MatchedPath>()?
                .as_str()
                .to_owned(),
            method: request.method().clone(),
            headers: request.headers().clone(),
            host: request_host(request.uri(), request.headers()),
            from_this_machine: crate::client::from_this_machine(
                request.extensions(),
                request.headers(),
            ),
        })
    }
}

/// The scopes this request actually carries.
///
/// A session, or a caller on this machine while the login is switched off, holds every API
/// scope — that is what an interactive administrator is. A bearer holds what was minted into
/// it, expanded through the implication edges, which is where a legacy `api:*` becomes the
/// full set. `from_this_machine` is `client::from_this_machine` of the same request.
pub async fn granted_scopes(
    state: &AppState,
    headers: &HeaderMap,
    from_this_machine: bool,
) -> Vec<rd_core::Scope> {
    credential(state, headers, from_this_machine).await.0
}

/// The scopes this request carries **and who is carrying them**.
///
/// One lookup answers both, which is the point: the audit log names an actor (RD-110-03), and
/// asking the database a second time to find out who it was would let the two answers
/// disagree about the same request. The token's bearer value is never read here — only its
/// digest is, and only to find the row; the id and the label that come back are the two
/// things a person already sees in the token list.
pub async fn credential(
    state: &AppState,
    headers: &HeaderMap,
    from_this_machine: bool,
) -> (Vec<rd_core::Scope>, crate::audit::Actor) {
    let resolved = resolve(state, headers, from_this_machine, AuditChannel::Rest).await;
    (resolved.scopes, resolved.actor)
}

/// What a request's credential resolved to: [`credential`]'s answer, and the call limit of
/// the token behind it (RD-1200-04).
pub(super) struct Resolved {
    pub(super) scopes: Vec<rd_core::Scope>,
    pub(super) actor: crate::audit::Actor,
    limit: Option<(rd_core::CaptureTokenId, u32)>,
}

impl Resolved {
    fn of(scopes: Vec<rd_core::Scope>, actor: crate::audit::Actor) -> Self {
        Self {
            scopes,
            actor,
            limit: None,
        }
    }

    /// Counts this call against the token's limit, or the `429` it earns.
    pub(super) fn admit(&self) -> Result<(), super::token_rate::Refused> {
        match self.limit {
            Some((token, limit)) => super::token_rate::admit(token, Some(limit)),
            None => Ok(()),
        }
    }
}

/// [`credential`], with the door the request came through (RD-1200-04): the actor carries
/// it, and so does the throttled `token_used` record.
pub(super) async fn resolve(
    state: &AppState,
    headers: &HeaderMap,
    from_this_machine: bool,
    via: AuditChannel,
) -> Resolved {
    let full = rd_core::Scope::API.to_vec();
    if state.auth.disabled_for(from_this_machine) {
        // Nobody signed in, and every caller on this machine is an administrator. "Anonymous"
        // is the honest word for that, and it is worth being able to filter an audit log by it.
        return Resolved::of(full, crate::audit::Actor::anonymous().through(via));
    }
    if let Some(session) = state.auth.current_session(state, headers).await {
        return Resolved::of(
            full,
            crate::audit::Actor::session(session.id.to_string()).through(via),
        );
    }
    let Some(token) = bearer_token(headers) else {
        return Resolved::of(Vec::new(), crate::audit::Actor::anonymous().through(via));
    };
    let digest = digest_of(token);
    match state.database.capture_token_identity(&digest).await {
        Ok(Some((id, label, scopes, calls_per_minute))) => {
            note_token_use(state, digest.clone());
            note_token_audit(state, id, label.clone(), scopes.clone(), via);
            Resolved {
                scopes: rd_core::granted_scopes(scopes.iter().map(String::as_str)),
                actor: crate::audit::Actor::token(id.to_string(), label).through(via),
                limit: calls_per_minute.map(|limit| (id, limit)),
            }
        }
        // A token nobody can look up grants nothing; a database error must not grant more
        // than a missing token does.
        Ok(None) => Resolved::of(Vec::new(), crate::audit::Actor::anonymous().through(via)),
        Err(error) => {
            tracing::warn!(error = %error, "could not read token scopes for the policy check");
            Resolved::of(Vec::new(), crate::audit::Actor::anonymous().through(via))
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
pub(super) static TOKEN_USE_SEEN: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<rd_core::CaptureTokenId, chrono::DateTime<chrono::Utc>>,
    >,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Records that a token was accepted, at most once per interval.
///
/// Detached, unlike every other audit write: this is not an action somebody asked for, it is
/// an observation about a request that is already being served, and making that request wait
/// for a database write would put the audit log in the path of every machine client.
pub(super) fn note_token_audit(
    state: &AppState,
    id: rd_core::CaptureTokenId,
    label: String,
    scopes: Vec<String>,
    via: AuditChannel,
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
                .actor(crate::audit::Actor::token(id.to_string(), label).through(via))
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
pub async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Captured before anything is awaited; see [`RequestFacts`].
    let facts = RequestFacts::of(&request);
    if let Some(facts) = facts.as_ref() {
        refuse_foreign_site(&state, &facts.method, facts.host.as_deref(), &facts.headers).await?;
    }
    let (granted, actor) = match facts.as_ref() {
        Some(facts) => match local_control_grant(&state, facts) {
            Some(grant) => grant,
            None => {
                let resolved = resolve(
                    &state,
                    &facts.headers,
                    facts.from_this_machine,
                    AuditChannel::Rest,
                )
                .await;
                // Before the scope check: a call over the limit is refused whatever it asks
                // for (RD-1200-04).
                if let Err(refused) = resolved.admit() {
                    return Ok(refused.into_response());
                }
                (resolved.scopes, resolved.actor)
            }
        },
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

/// The name a request gives this service: its `Host`, else the URI authority -- an HTTP/2
/// request may carry only `:authority` (audit 1.9.1, RA-API-05). The same two `host_check`
/// reads, with the port kept, because an origin is compared port and all.
#[must_use]
pub fn request_host(uri: &axum::http::Uri, headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(header::HOST) {
        return Some(value.to_str().unwrap_or("null").to_owned());
    }
    let authority = uri.authority()?;
    Some(match authority.port() {
        Some(port) => format!("{}:{}", authority.host(), port.as_str()),
        None => authority.host().to_owned(),
    })
}

/// Refuses a state-changing request a browser sent from a page of another site (audit 1.9.1,
/// API-01); the decision is `rd_authn::origin`.
///
/// Before any credential is looked at, on purpose: the credential a cross-site request carries
/// is the browser's ambient one -- and none at all when the login is switched off for this
/// machine, where every caller here is the administrator. A safe method changes nothing and
/// passes; so does a request without `Origin` and `Sec-Fetch-Site`, which no browser sends.
/// `host` is [`request_host`] of the request.
pub async fn refuse_foreign_site(
    state: &AppState,
    method: &Method,
    host: Option<&str>,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if method.is_safe() {
        return Ok(());
    }
    refuse_foreign(state, host, headers, false).await
}

/// [`refuse_foreign_site`] for a request whose credential is a cookie the browser adds by
/// itself, on every method: the qBittorrent adapter changes state on `GET` as qBittorrent does,
/// and its `SID` cookie is `SameSite=Strict`, which still rides along from a page on another
/// port of the same name (audit 1.9.1, RA-API-02). Such a page's `GET` -- an image, a link --
/// carries no `Origin`, so a browser that says `same-site` without one is refused here too;
/// `same-origin` and `none` (typed into the address bar) pass, and so does any client that
/// sends neither header, which is every *arr.
pub async fn refuse_foreign_cookie_request(
    state: &AppState,
    host: Option<&str>,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    refuse_foreign(state, host, headers, true).await
}

pub(super) async fn refuse_foreign(
    state: &AppState,
    host: Option<&str>,
    headers: &HeaderMap,
    ambient_credential: bool,
) -> Result<(), ApiError> {
    let text = |name: header::HeaderName| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap_or("null").to_owned())
    };
    let sec_fetch_site = text(header::HeaderName::from_static("sec-fetch-site"));
    let origin = text(header::ORIGIN);
    if sec_fetch_site.is_none() && origin.is_none() {
        return Ok(());
    }
    let unvouched = ambient_credential
        && origin.is_none()
        && sec_fetch_site.as_deref().is_some_and(|site| {
            let site = site.trim();
            !site.eq_ignore_ascii_case("same-origin") && !site.eq_ignore_ascii_case("none")
        });
    let external_origin = state.proxy.read().await.origin().map(str::to_owned);
    let foreign = rd_authn::origin::foreign_request(
        sec_fetch_site.as_deref(),
        origin.as_deref(),
        host,
        external_origin.as_deref(),
    )
    .or(unvouched.then_some(rd_authn::origin::Foreign::Origin));
    let Some(foreign) = foreign else {
        return Ok(());
    };
    tracing::warn!(
        origin = origin.as_deref().unwrap_or("-"),
        sec_fetch_site = sec_fetch_site.as_deref().unwrap_or("-"),
        reason = ?foreign,
        "a state-changing request from another site was refused"
    );
    Err(ApiError::forbidden(
        "request.cross_site_refused",
        "This service does not accept changes sent from a page of another site",
    ))
}

/// The local control token (`crate::local_control`, RD-180-02) opens its own routes and no
/// other, and only for a request from this machine: `api:admin` on exactly those, which is what
/// the table prices them at. Any other request falls through to the ordinary credential.
pub(super) fn local_control_grant(
    state: &AppState,
    facts: &RequestFacts,
) -> Option<(Vec<rd_core::Scope>, crate::audit::Actor)> {
    let bearer = bearer_token(&facts.headers)?;
    (facts.from_this_machine
        && crate::local_control::covers(&facts.path, &facts.method)
        && state.local_control.accepts(bearer))
    .then(|| {
        (
            vec![rd_core::Scope::Admin],
            crate::audit::Actor::local_control(),
        )
    })
}

/// The refusal this route's requirement produces for these scopes, if any.
pub(super) fn scope_refusal(
    facts: Option<&RequestFacts>,
    granted: &[rd_core::Scope],
) -> Option<ApiError> {
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
