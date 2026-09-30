//! Whether the credential an event stream was opened with still stands (security audit
//! 2026-09-30, finding 4).
//!
//! A request is authorised once, when it arrives. For every other route that is the whole
//! story; for the two event streams it is not, because they stay open for hours. Signing out,
//! revoking a token, a password change that ends every session, a session running into its
//! limit — none of it reached a stream that was already open, which went on delivering the bus
//! to a credential that no longer existed. So a stream asks again, every
//! [`AppState::stream_recheck`](crate::AppState), and ends once the answer is no: the client
//! reconnects, and the reconnect is authorised like any other request.
//!
//! The lookups here record nothing. A session's `last_used_at` is what its idle limit is
//! measured from, and a tab left open must not keep a session alive by being open.

use std::{future::Future, time::Duration};

use axum::http::HeaderMap;
use rd_core::Scope;

use crate::{AppState, auth};

/// How often an open stream checks its credential by default: well inside a minute, and rare
/// enough to cost nothing next to the events it carries.
pub const STREAM_RECHECK: Duration = Duration::from_secs(30);

/// The API scopes the credential on `headers` carries now.
///
/// The same answer `auth::credential` gives, read without touching the session, the token's
/// last use or the audit log. A store that cannot be read answers "nothing": the stream ends
/// and its reconnect meets the ordinary check.
pub async fn standing_scopes(
    state: &AppState,
    headers: &HeaderMap,
    from_this_machine: bool,
) -> Vec<Scope> {
    if state.auth.disabled_for(from_this_machine) {
        return Scope::API.to_vec();
    }
    if let Some(token) = auth::session_token(headers) {
        match state
            .database
            .session_for_digest(&auth::digest_of(token), state.auth.session_limits())
            .await
        {
            Ok(Some(_)) => return Scope::API.to_vec(),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %error, "could not re-check an event stream's session");
                return Vec::new();
            }
        }
    }
    let Some(token) = auth::bearer_token(headers) else {
        return Vec::new();
    };
    match state
        .database
        .capture_token_scopes(&auth::digest_of(token))
        .await
    {
        Ok(Some(scopes)) => rd_core::granted_scopes(scopes.iter().map(String::as_str)),
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::warn!(error = %error, "could not re-check an event stream's token");
            Vec::new()
        }
    }
}

/// Completes once the credential on `headers` no longer carries every scope in `opened_with`:
/// signed out, revoked, expired, or narrowed below what the stream filters for.
pub async fn api_lapsed(
    state: AppState,
    headers: HeaderMap,
    from_this_machine: bool,
    opened_with: Vec<Scope>,
) {
    let (state, headers, opened_with) = (&state, &headers, &opened_with);
    until_lapsed(state.stream_recheck, move || async move {
        let now = standing_scopes(state, headers, from_this_machine).await;
        opened_with.iter().all(|scope| now.contains(scope))
    })
    .await;
    tracing::info!("an event stream's credential lapsed; the stream ends");
}

/// Completes once the capture token on `headers` is no longer a live capture token.
pub async fn capture_lapsed(state: AppState, headers: HeaderMap) {
    let (state, headers) = (&state, &headers);
    until_lapsed(state.stream_recheck, move || async move {
        let Some(token) = auth::bearer_token(headers) else {
            return false;
        };
        state
            .database
            .capture_token_valid(&auth::digest_of(token), rd_core::CAPTURE_SCOPE)
            .await
            .unwrap_or(false)
    })
    .await;
    tracing::info!("a capture stream's token lapsed; the stream ends");
}

/// Asks `stands` every `every`, the first time one interval after the stream opened, and
/// returns the first time it says no.
async fn until_lapsed<F, Fut>(every: Duration, mut stands: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if !stands().await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::until_lapsed;

    /// The loop asks until the answer turns, and returns on the first no.
    #[tokio::test]
    async fn the_loop_ends_on_the_first_refusal() {
        let asked = AtomicUsize::new(0);
        let counter = &asked;
        until_lapsed(Duration::from_millis(1), move || async move {
            counter.fetch_add(1, Ordering::SeqCst) < 2
        })
        .await;
        assert_eq!(asked.load(Ordering::SeqCst), 3);
    }
}
