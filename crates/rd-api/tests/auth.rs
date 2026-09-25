//! Changing the administrator password (RD-120-22).
//!
//! The job this file exists for is narrow and the traps are all in the refusals, so most of
//! what is asserted here is what a refusal must *not* reveal:
//!
//! * A wrong current password and a wrong sign-in must be one response, byte for byte.
//! * The policy refusal for the replacement must not depend on whether the current password
//!   was right, or it becomes a password oracle behind a route a machine token can reach.
//! * The audit record must carry neither password, in either direction.
//!
//! The positive direction is one test; the negative direction is the rest of the file.

mod common;

use axum::http::StatusCode;
use common::{
    auth_harness, get_with_cookie, post_json, post_json_with_cookie,
    post_json_with_cookie_and_headers, post_json_with_headers,
};

const PASSWORD: &str = "correct-horse-battery";
const REPLACEMENT: &str = "a-quite-different-passphrase";
const WRONG: &str = "not-the-password-at-all";

/// Completes setup and signs in, returning the session token.
async fn sign_in(harness: &common::Harness) -> String {
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert!(
        status.is_success() || body["code"] == "auth.setup_completed",
        "setup: {body}"
    );
    log_in(harness, PASSWORD).await.expect("a session")
}

/// Signs in with `password`, returning the session token if it was accepted.
async fn log_in(harness: &common::Harness, password: &str) -> Option<String> {
    let (_, _, token) = post_json_with_headers(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": password }),
    )
    .await;
    token
}

/// `POST /api/v1/auth/password` with a session cookie.
async fn change(
    harness: &common::Harness,
    token: &str,
    current: &str,
    new: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    post_json_with_cookie_and_headers(
        &harness.router,
        "/api/v1/auth/password",
        token,
        serde_json::json!({ "current_password": current, "new_password": new }),
    )
    .await
}

#[tokio::test]
async fn the_password_can_be_changed_with_the_current_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body, _) = change(&harness, &token, PASSWORD, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "auth.password_changed");

    assert!(
        log_in(&harness, PASSWORD).await.is_none(),
        "the old password still opens a session"
    );
    assert!(
        log_in(&harness, REPLACEMENT).await.is_some(),
        "the new password does not open a session"
    );
}

/// The refusal an attacker sees must be one response, not two.
///
/// Compared whole rather than field by field: a later addition that only appears on one of
/// the two paths -- a hint, a parameter, a different message for the same code -- is exactly
/// the leak this guards, and a field-by-field assertion would not see it.
#[tokio::test]
async fn a_wrong_current_password_is_refused_exactly_like_a_wrong_sign_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (change_status, change_body, _) = change(&harness, &token, WRONG, REPLACEMENT).await;
    let (login_status, login_body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": WRONG }),
    )
    .await;

    assert_eq!(change_status, StatusCode::UNAUTHORIZED, "{change_body}");
    assert_eq!(change_status, login_status);
    assert_eq!(
        change_body, login_body,
        "a wrong current password is distinguishable from a wrong sign-in"
    );

    assert!(
        log_in(&harness, PASSWORD).await.is_some(),
        "a refused change replaced the password anyway"
    );
}

/// The oracle the ordering inside the handler exists to close.
///
/// If the current password were checked first, `auth.password_too_short` would only come back
/// when it was *right*, and anybody who can reach this route could then test passwords by
/// sending a deliberately short replacement and reading which refusal came back.
#[tokio::test]
async fn the_policy_refusal_does_not_say_whether_the_current_password_was_right() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (right_status, right_body, _) = change(&harness, &token, PASSWORD, "short").await;
    let (wrong_status, wrong_body, _) = change(&harness, &token, WRONG, "short").await;

    assert_eq!(right_status, StatusCode::BAD_REQUEST, "{right_body}");
    assert_eq!(right_body["code"], "auth.password_too_short");
    assert_eq!(right_status, wrong_status);
    assert_eq!(
        right_body, wrong_body,
        "a short replacement told the caller whether the current password was right"
    );

    assert!(
        log_in(&harness, PASSWORD).await.is_some(),
        "a refused change replaced the password anyway"
    );
}

/// `validate_password` governs the replacement exactly as it governed the first one.
#[tokio::test]
async fn the_replacement_is_held_to_the_password_policy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    for attempt in ["", "x", "123456789"] {
        let (status, body, _) = change(&harness, &token, PASSWORD, attempt).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{attempt:?}: {body}");
        assert_eq!(body["code"], "auth.password_too_short", "{attempt:?}");
        assert_eq!(body["params"]["min"], "10", "{attempt:?}");
    }
    // Ten characters is the floor, so ten characters is accepted.
    let (status, body, _) = change(&harness, &token, PASSWORD, "0123456789").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A change that changes nothing is the worst outcome on a screen somebody reached because
/// the password in force is the one that leaked.
#[tokio::test]
async fn the_replacement_must_differ_from_the_current_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body, _) = change(&harness, &token, PASSWORD, PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "auth.password_unchanged");
}

/// Every session ends -- and the caller stays signed in anyway.
///
/// Both halves matter. A change that leaves the sessions opened with the old password alive
/// protects against nothing, and one that also signs the caller out of the screen they made
/// it on looks like a failure and invites them to change it back.
#[tokio::test]
async fn the_change_ends_every_session_and_hands_the_caller_a_fresh_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let here = sign_in(&harness).await;
    let elsewhere = log_in(&harness, PASSWORD).await.expect("a second session");

    let (status, body, fresh) = change(&harness, &here, PASSWORD, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["params"]["sessions_ended"], "2", "{body}");

    for (name, token) in [("the other device", &elsewhere), ("the old cookie", &here)] {
        let (status, body) = get_with_cookie(&harness.router, "/api/v1/sessions", token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name} survived: {body}");
        assert_eq!(body["code"], "auth.session_required", "{name}");
    }

    let fresh = fresh.expect("the caller was not handed a new session");
    let (status, body) = get_with_cookie(&harness.router, "/api/v1/sessions", &fresh).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the fresh session does not work: {body}"
    );
    assert_eq!(
        body.as_array().expect("sessions").len(),
        1,
        "exactly the caller's own session should be left: {body}"
    );
}

/// The limiter is the sign-in's, used rather than rebuilt.
///
/// Two things are asserted, and the second is the one that matters. The endpoint locks out a
/// guesser -- and the lockout it hands back is *bounded*, so it can never become permanent.
/// That it is per address and expires on its own is held where the curve lives, in
/// `rd_authn::throttle`'s `one_address_being_locked_out_does_not_lock_out_another` and
/// `a_lockout_ends_when_its_time_is_up`; reimplementing that here would mean reimplementing
/// the limiter here, which is the thing this route deliberately does not do.
#[tokio::test]
async fn repeated_wrong_current_passwords_are_locked_out_like_repeated_wrong_sign_ins() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let mut locked = None;
    for _ in 0..8 {
        let (status, body, _) = change(&harness, &token, WRONG, REPLACEMENT).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            locked = Some(body);
            break;
        }
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
    let locked = locked.expect("guessing was never locked out");
    assert_eq!(locked["code"], "auth.too_many_attempts");
    let seconds: u64 = locked["params"]["seconds"]
        .as_str()
        .expect("seconds")
        .parse()
        .expect("a number");
    assert!(
        (1..=15 * 60).contains(&seconds),
        "a lockout of {seconds}s is outside the bound that keeps it from becoming permanent"
    );

    // One limiter, not two: the failures counted here are the failures the sign-in counts.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "auth.too_many_attempts");
}

/// The record says who changed it and what it cost, and neither password is anywhere in it.
#[tokio::test]
async fn the_change_is_recorded_without_either_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body, _) = change(&harness, &token, WRONG, REPLACEMENT).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, body, _) = change(&harness, &token, PASSWORD, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let stored = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 500,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("query");
    let records: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .collect();

    let changed = records
        .iter()
        .find(|record| record["action"] == "password_changed" && record["outcome"] == "success")
        .unwrap_or_else(|| panic!("the change was not recorded: {records:?}"));
    assert_eq!(changed["actor_kind"], "session");
    assert_eq!(changed["details"]["sessions_ended"], "1");

    let refused = records
        .iter()
        .find(|record| record["action"] == "password_changed" && record["outcome"] == "failure")
        .unwrap_or_else(|| panic!("the refused change was not recorded: {records:?}"));
    // The *stage* that refused, never the value that was wrong.
    assert_eq!(refused["details"]["stage"], "current_password");

    let whole = serde_json::to_string(&records).expect("json");
    for canary in [PASSWORD, REPLACEMENT, WRONG] {
        assert!(
            !whole.contains(canary),
            "a password reached the audit log: {canary}"
        );
    }
}

/// The new way in is a second door, not a widened first one.
#[tokio::test]
async fn setup_still_refuses_its_second_call_after_a_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (status, body, _) = change(&harness, &token, PASSWORD, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": "a-third-password-entirely" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "auth.setup_completed");
    assert!(
        log_in(&harness, "a-third-password-entirely")
            .await
            .is_none(),
        "setup wrote a password over an installation that already had one"
    );
    assert!(log_in(&harness, REPLACEMENT).await.is_some());
}

/// The inverse of the TOTP encoder, so a test can produce the code an app would show.
fn base32_decode(value: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0_u32;
    let mut accumulated = 0_u64;
    let mut out = Vec::new();
    for character in value.bytes() {
        let index = ALPHABET
            .iter()
            .position(|entry| *entry == character)
            .expect("base32") as u64;
        accumulated = (accumulated << 5) | index;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulated >> bits) & 0xFF) as u8);
        }
    }
    out
}

fn current_code(secret_base32: &str) -> String {
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    rd_authn::totp::code_at(&base32_decode(secret_base32), now)
}

/// A configured second factor is not asked for again here, and is not touched by the change.
///
/// The answer the job file argues: the factor was already weighed when the session that
/// reached this route was opened, so demanding a code again would be a step-up prompt, which
/// is a separate decision. What must not happen is the change quietly *removing* the factor
/// or the recovery codes, and that is the half this pins.
#[tokio::test]
async fn a_configured_second_factor_is_not_demanded_again_and_survives_the_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/totp",
        &token,
        serde_json::json!({ "label": "Phone" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let credential = body["credential_id"].as_str().expect("id").to_owned();
    let secret = body["secret"].as_str().expect("secret").to_owned();
    let (status, body) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{credential}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;
    assert!(status.is_success(), "{body}");

    // No `code` field anywhere in the request, and it is accepted.
    let (status, body, _) = change(&harness, &token, PASSWORD, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The factor is still there and still gates the sign-in, with the new password.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": REPLACEMENT }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        body["code"], "auth.mfa_required",
        "the password change switched the second factor off"
    );
    assert!(
        log_in(&harness, PASSWORD).await.is_none(),
        "the old password still opens a session"
    );
}
