//! A call limit per API token (RD-1200-04, `docs/security/mcp.md` finding 8).
//!
//! A token may carry a number of calls per minute. Every request it is accepted on counts one
//! call — a REST request, an MCP message, a compatibility client's poll — and the call above
//! the limit is refused with `429 api.token_rate_limited` and a `Retry-After`, which is the
//! refusal every HTTP and MCP client already understands. A fixed window per token, started by
//! its first call: simple to reason about, and a burst at a window's edge is at most twice the
//! limit, which is all a limit against a runaway agent has to hold.
//!
//! In memory on purpose, like the token-use throttles beside it: after a restart every token
//! starts a fresh window, which costs at most one extra minute's calls.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};

use crate::ApiError;

/// The window a limit counts over.
const WINDOW: Duration = Duration::from_secs(60);

/// Windows kept before the expired ones are swept; bounded by the live tokens in practice.
const SWEEP_ABOVE: usize = 4096;

struct Window {
    started: Instant,
    calls: u32,
}

/// The open window of every limited token.
#[derive(Default)]
struct Windows(HashMap<rd_core::CaptureTokenId, Window>);

impl Windows {
    /// Counts one call of `token` at `now` under `limit`, or answers how long until its window
    /// ends when the call is one too many. A refused call is not counted: a client that backs
    /// off as told gets its full allowance in the next window.
    fn admit(
        &mut self,
        token: rd_core::CaptureTokenId,
        limit: u32,
        now: Instant,
    ) -> Result<(), Duration> {
        if self.0.len() >= SWEEP_ABOVE {
            self.0
                .retain(|_, window| now.duration_since(window.started) < WINDOW);
        }
        let window = self.0.entry(token).or_insert(Window {
            started: now,
            calls: 0,
        });
        if now.duration_since(window.started) >= WINDOW {
            *window = Window {
                started: now,
                calls: 0,
            };
        }
        if window.calls >= limit {
            return Err(WINDOW.saturating_sub(now.duration_since(window.started)));
        }
        window.calls += 1;
        Ok(())
    }
}

static WINDOWS: LazyLock<Mutex<Windows>> = LazyLock::new(|| Mutex::new(Windows::default()));

/// A call above its token's limit: the limit, and how long until the window ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Refused {
    limit: u32,
    left: Duration,
}

impl Refused {
    /// Whole seconds to wait, at least one: what `Retry-After` says.
    pub(super) fn seconds(self) -> u64 {
        (self.left.as_secs() + u64::from(self.left.subsec_nanos() > 0)).max(1)
    }

    /// The `429`: the stable code, the limit and the seconds to wait, in the body for a person
    /// and in `Retry-After` for a client.
    pub(super) fn into_response(self) -> Response {
        refusal(self.limit, self.seconds())
    }
}

/// Counts one call of `token`, or says why it is one too many. A token without a limit is
/// never counted. A poisoned lock admits: the limit is a brake on a runaway client, not an
/// authorisation, and the scopes have already decided what the call may reach.
pub(super) fn admit(
    token: rd_core::CaptureTokenId,
    calls_per_minute: Option<u32>,
) -> Result<(), Refused> {
    let Some(limit) = calls_per_minute else {
        return Ok(());
    };
    let Ok(mut windows) = WINDOWS.lock() else {
        return Ok(());
    };
    windows
        .admit(token, limit, Instant::now())
        .map_err(|left| Refused { limit, left })
}

fn refusal(limit: u32, seconds: u64) -> Response {
    let mut response = ApiError::too_many_requests(
        "api.token_rate_limited",
        "This token has made more calls this minute than its limit allows",
    )
    .with_param("limit", limit)
    .with_param("seconds", seconds)
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::http::{StatusCode, header};

    use super::{Refused, WINDOW, Windows};

    #[test]
    fn the_call_above_the_limit_is_refused_and_the_next_window_admits_again() {
        let mut windows = Windows::default();
        let token = rd_core::CaptureTokenId::new();
        let start = Instant::now();
        for _ in 0..3 {
            assert!(windows.admit(token, 3, start).is_ok());
        }
        let left = windows
            .admit(token, 3, start + Duration::from_secs(20))
            .expect_err("the fourth call in a minute");
        assert_eq!(left, Duration::from_secs(40));
        assert!(windows.admit(token, 3, start + WINDOW).is_ok());
    }

    #[test]
    fn each_token_counts_on_its_own() {
        let mut windows = Windows::default();
        let (busy, quiet) = (
            rd_core::CaptureTokenId::new(),
            rd_core::CaptureTokenId::new(),
        );
        let now = Instant::now();
        assert!(windows.admit(busy, 1, now).is_ok());
        assert!(windows.admit(busy, 1, now).is_err());
        assert!(windows.admit(quiet, 1, now).is_ok());
    }

    #[test]
    fn the_refusal_is_a_429_with_its_code_and_a_retry_after() {
        let refused = Refused {
            limit: 30,
            left: Duration::from_millis(12_300),
        };
        assert_eq!(refused.seconds(), 13);
        let response = refused.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .map(|value| value.as_bytes()),
            Some(b"13".as_slice())
        );
        let no_wait = Refused {
            limit: 30,
            left: Duration::ZERO,
        }
        .into_response();
        assert_eq!(
            no_wait
                .headers()
                .get(header::RETRY_AFTER)
                .map(|value| value.as_bytes()),
            Some(b"1".as_slice())
        );
    }
}
