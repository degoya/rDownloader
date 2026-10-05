//! The administrator session store: opening, checking, listing and ending sessions, and the
//! limits that govern them.

use super::*;

impl AuthService {
    /// Whether the administrator login is switched off.
    #[must_use]
    pub fn disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Whether a request is let in without a credential: the login is switched off **and**
    /// the request comes from this machine (`client::from_this_machine`). Anybody else has to
    /// sign in as though the login were on.
    #[must_use]
    pub fn disabled_for(&self, from_this_machine: bool) -> bool {
        from_this_machine && self.disabled()
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
        admin_password_configured(&state.database).await
    }

    /// Stores the first administrator password, once.
    ///
    /// The early read is only the cheap refusal, so a configured installation does not hash a
    /// password for every anonymous call. What decides is the write: it only lands on an empty
    /// key, in one statement. A read followed by a write let two requests racing on a fresh
    /// installation both find it unconfigured, and the second password silently replaced the
    /// first (security audit 2026-09-30, finding 7).
    pub async fn setup(&self, state: &AppState, password: &str) -> Result<(), ApiError> {
        if self.is_configured(state).await? {
            return Err(setup_completed());
        }
        validate_password(password)?;
        let hash = hash_password(password).await?;
        if !state
            .database
            .insert_setting_if_absent(PASSWORD_SETTING.to_owned(), serde_json::Value::String(hash))
            .await?
        {
            return Err(setup_completed());
        }
        Ok(())
    }

    /// Writes `password` as the administrator password, whatever is there now.
    ///
    /// The one place that hashes, shared by [`Self::setup`] and the password change
    /// (RD-120-22) so the two cannot drift apart on the policy or on the hash parameters.
    /// It deliberately checks *no* credential: who may call it is the caller's question, and
    /// a function that both authorised and wrote would let the two disagree.
    pub async fn store_password(&self, state: &AppState, password: &str) -> Result<(), ApiError> {
        store_admin_password(&state.database, password).await
    }

    /// Runs `apply` on the limiter, created on first use.
    fn with_throttle<T>(&self, apply: impl FnOnce(&mut rd_authn::LoginThrottle) -> T) -> T {
        let mut guard = self
            .throttle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply(guard.get_or_insert_with(|| {
            rd_authn::LoginThrottle::new(rd_authn::ThrottleSettings::default())
        }))
    }

    /// What the limiter says about an attempt from `client`.
    pub async fn throttle_check(&self, client: std::net::IpAddr) -> rd_authn::Decision {
        self.with_throttle(|throttle| throttle.check(client, Instant::now()))
    }

    /// The limiter's gate for one attempt from `client`, asked once (audit 1.9.1, API-10).
    ///
    /// A locked-out address is refused with the coded `429`, carrying the seconds it has to
    /// wait; any other attempt waits out the global slow-down first. Every sign-in door goes
    /// through this, so none of them can honour the lockout and forget the delay -- the copies
    /// this replaces asked the limiter twice and each spelled the refusal out again.
    ///
    /// The limiter is asked again after the wait, and that second answer admits the attempt:
    /// the returned [`LoginAttempt`] counts against the address until it is dropped. Asked only
    /// before the wait, parallel attempts all slept through the lockout the first of them
    /// caused, and all reached the password check (audit 2026-10-05, S4).
    ///
    /// # Errors
    ///
    /// `429 auth.too_many_attempts` with the `seconds` parameter.
    pub async fn gate(&self, client: std::net::IpAddr) -> Result<LoginAttempt, ApiError> {
        if let rd_authn::Decision::Proceed { delay } = self.throttle_check(client).await
            && !delay.is_zero()
        {
            // Paid by everyone while an attack is running, and capped low enough that it
            // stays a nuisance rather than an outage.
            tokio::time::sleep(delay).await;
        }
        match self.with_throttle(|throttle| throttle.admit(client, Instant::now())) {
            rd_authn::Decision::Locked { retry_after } => Err(ApiError::too_many_requests(
                "auth.too_many_attempts",
                "Too many failed sign-in attempts from this address",
            )
            .with_param("seconds", retry_after.as_secs().max(1).to_string())),
            rd_authn::Decision::Proceed { .. } => Ok(LoginAttempt {
                throttle: self.throttle.clone(),
                client,
            }),
        }
    }

    pub(super) async fn record_login_failure(&self, client: std::net::IpAddr) {
        self.with_throttle(|throttle| throttle.record_failure(client, Instant::now()));
    }

    pub(super) async fn record_login_success(&self, client: std::net::IpAddr) {
        self.with_throttle(|throttle| throttle.record_success(client, Instant::now()));
    }

    /// Forgets every failure the limiter counted, every address's lockout included.
    ///
    /// For the password reset on the host (RD-190-24): the owner who forgot the password has
    /// usually locked their own address out trying, and a new password they cannot try for a
    /// quarter of an hour would be no way back in. One account, so the whole limiter.
    pub async fn reset_throttle(&self) {
        *self
            .throttle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
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
        admin_password_matches(&state.database, password).await
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

    /// The `Set-Cookie` value that clears the session cookie.
    ///
    /// At the path [`Self::cookie`] sets it at: a browser keys a cookie by name *and* path, so
    /// clearing `Path=/` under a mount point left the session cookie of `Path=/downloads` where it
    /// was (security audit 2026-09-30, finding 6). Deliberately without `Secure`: clearing a
    /// cookie has to work whatever the current configuration says, including after the
    /// configuration changed under it.
    #[must_use]
    pub fn expired_cookie(base_path: &str) -> String {
        let path = if base_path.is_empty() { "/" } else { base_path };
        format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path={path}; Max-Age=0")
    }

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
