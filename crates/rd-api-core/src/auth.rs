use std::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Instant,
};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;

use crate::{ApiError, AppState, scope_policy};

mod middleware;
mod password;
mod service;
mod tokens;

pub use middleware::*;
pub use password::*;
pub use tokens::*;

const PASSWORD_SETTING: &str = "auth.admin_password_hash";

/// The scopes a request carries, for handlers that narrow their own output.
///
/// Replaces the former `AccessLevel` two-value enum, which could only say "everything" or
/// "the read allowlist". The event stream needs to know *which* areas a subscriber may see,
/// not how privileged it feels.
#[derive(Clone, Debug, Default)]
pub struct Granted(pub(crate) Vec<rd_core::Scope>);

impl Granted {
    /// Whether this credential carries `scope`.
    pub fn holds(&self, scope: rd_core::Scope) -> bool {
        self.0.contains(&scope)
    }

    /// Every scope this credential carries, for a stream that has to notice losing one.
    pub fn scopes(&self) -> &[rd_core::Scope] {
        &self.0
    }

    /// Whether an event of this kind may be streamed to this subscriber.
    pub fn may_observe(&self, kind: &rd_core::EventKind) -> bool {
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
    /// A plain mutex: no caller holds it across an `.await`, and a [`LoginAttempt`] has to
    /// release its slot from `Drop`, where nothing can be awaited.
    throttle: SharedThrottle,
}

type SharedThrottle = std::sync::Arc<std::sync::Mutex<Option<rd_authn::LoginThrottle>>>;

/// One sign-in attempt [`AuthService::gate`] let in (audit 2026-10-05, S4).
///
/// While it is held the attempt counts against its address as though it had already failed, so
/// parallel attempts cannot all pass the gate before the first failure is counted. Hold it until
/// the verdict is recorded; dropping it hands the slot back.
#[must_use = "an attempt dropped at once is not counted while its credential is checked"]
pub struct LoginAttempt {
    throttle: SharedThrottle,
    client: std::net::IpAddr,
}

impl Drop for LoginAttempt {
    fn drop(&mut self) {
        let mut guard = self
            .throttle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A reset in between left nothing to hand back to.
        if let Some(throttle) = guard.as_mut() {
            throttle.release(self.client);
        }
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
