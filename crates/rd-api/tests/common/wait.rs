//! Waiting for a condition instead of for a clock.
//!
//! A fixed sleep before an assertion is a guess at how long the system needs, and the guess is
//! wrong on the one runner that is slower than the machine it was made on. Twenty-seven hand
//! written loops each carried their own budget, their own interval and their own message, and
//! several ended by falling through to an assertion instead of failing, so "never happened"
//! and "happened wrongly" read the same (RD-140-21). Everything that waits goes through
//! [`eventually`] now.
//!
//! Waiting for something *not* to happen is not something this module offers, on purpose: a
//! test that checks for an absence first waits for a witness — an event, a row, a counter the
//! same step produces — so that "nothing happened" is concluded only once the step is known
//! to have been processed.

use std::{future::Future, time::Duration};

use axum::Router;

/// How often a condition is asked again.
const POLL: Duration = Duration::from_millis(25);

/// The budget most waits use: long enough for a loaded runner, short enough to fail a run
/// that hangs while somebody is still looking at it.
pub const WAIT: Duration = Duration::from_secs(10);

/// Asks `probe` until it answers `Some`, and returns what it answered.
///
/// Fails the test with `what` when `within` passes first. `probe` is asked once more after
/// the deadline has passed, never less than once, so a slow first call is not a failure.
pub async fn eventually<T, Fut>(within: Duration, what: &str, mut probe: impl FnMut() -> Fut) -> T
where
    Fut: Future<Output = Option<T>>,
{
    eventually_ok(within, what, move || {
        let answer = probe();
        async move { answer.await.ok_or("not yet") }
    })
    .await
}

/// Asks `probe` until it answers `Ok`, and returns what it answered.
///
/// For a wait whose failure should say what was last seen: the last `Err` goes into the
/// message.
pub async fn eventually_ok<T, E, Fut>(
    within: Duration,
    what: &str,
    mut probe: impl FnMut() -> Fut,
) -> T
where
    E: std::fmt::Display,
    Fut: Future<Output = Result<T, E>>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match probe().await {
            Ok(value) => return value,
            Err(last) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{what}: not within {within:?}; last seen: {last}"
                );
                tokio::time::sleep(POLL).await;
            }
        }
    }
}

/// Waits until no candidate is still `checking` or `resolving`.
///
/// Intake starts an online check for every fresh link, and a package cannot be enqueued
/// while one is in flight.
///
/// The budget below is for a name that **does not resolve**: the check gets its refusal from
/// the resolver and settles at once. That only holds while every address a test hands in is
/// a reserved documentation name -- `*.example`, `*.test`, or a subdomain of `example.com`.
/// The bare `example.com` is not one of those: IANA answers for it, so a test using it does
/// real resolution and a real connection attempt, and under a loaded machine five seconds is
/// not enough. That is exactly how
/// `storage_capacity::a_root_is_released_again_once_its_threshold_fits` failed on 2026-09-22,
/// passing in 3.9s alone and timing out at 9.29s inside a full run (RD-120-26).
///
/// So the limit is not the thing to raise when this panics. Look at the address the test
/// handed in first -- a longer wait would only move the coin flip.
pub async fn wait_for_candidates_ready(router: &Router) {
    eventually(
        Duration::from_secs(5),
        "candidates never left the checking state",
        || async move {
            let (_, candidates) = super::get_json(router, "/api/v1/collector/candidates").await;
            let busy = candidates.as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| matches!(item["state"].as_str(), Some("checking" | "resolving")))
            });
            (!busy).then_some(())
        },
    )
    .await;
}
