//! Signing in, seeing what is signed in, and signing it out again.
//!
//! Runs against a harness with the administrator login switched **on**. With it disabled every
//! request is waved through before a session is consulted, which is exactly the configuration
//! that would make these pass without the feature existing.

mod common;

use axum::http::StatusCode;
use common::{auth_harness, get_json, post_json};
use sha2::{Digest, Sha256};

const PASSWORD: &str = "correct-horse-battery";

/// Completes setup and signs in, returning the session bearer.
async fn sign_in(harness: &common::Harness) -> String {
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert!(
        status.is_success() || body["code"] == "auth.setup_completed",
        "setup: {status} {body}"
    );
    let (status, _, cookie) = common::post_json_with_headers(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login");
    cookie.expect("a session cookie")
}

/// A session survives a restart, which the in-memory map it replaces could not do.
#[tokio::test]
async fn a_session_still_authenticates_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let token = {
        let harness = auth_harness(directory.path()).await;
        sign_in(&harness).await
    };

    // Reopening the same directory builds a second router over the same database.
    let harness = auth_harness(directory.path()).await;
    let (status, body) = common::get_with_cookie(&harness.router, "/api/v1/settings", &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the session did not survive the restart: {body}"
    );
}

#[tokio::test]
async fn the_inventory_shows_the_calling_session_as_current() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body) = common::get_with_cookie(&harness.router, "/api/v1/sessions", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sessions = body.as_array().expect("an array");
    assert_eq!(sessions.len(), 1, "{body}");
    assert_eq!(sessions[0]["current"], true, "{body}");
    // The bearer must never come back out of the inventory.
    assert!(
        !body.to_string().contains(&token),
        "the session list echoed the bearer back"
    );
}

/// The action exists to end sessions somebody does not recognise; ending their own with them
/// would make it unusable for the case it is for.
#[tokio::test]
async fn signing_out_everywhere_else_keeps_the_caller_signed_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let mine = sign_in(&harness).await;
    let other = sign_in(&harness).await;

    let (status, body) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/sessions/revoke-others",
        &mine,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["params"]["count"], "1", "{body}");

    let (status, _) = common::get_with_cookie(&harness.router, "/api/v1/settings", &mine).await;
    assert_eq!(status, StatusCode::OK, "the caller signed themselves out");
    let (status, _) = common::get_with_cookie(&harness.router, "/api/v1/settings", &other).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the other session survived"
    );
}

#[tokio::test]
async fn signing_out_ends_the_session_immediately() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, _) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/auth/logout",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = common::get_with_cookie(&harness.router, "/api/v1/settings", &token).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the session outlived the sign-out"
    );
}

/// Signing out without a session is a no-op, not an error: a browser holding a lapsed cookie
/// has to be able to clear it.
#[tokio::test]
async fn signing_out_without_a_session_succeeds_quietly() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, _) = post_json(
        &harness.router,
        "/api/v1/auth/logout",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Sustained guessing is refused, and says so rather than reporting a wrong password.
#[tokio::test]
async fn repeated_wrong_passwords_are_throttled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let _ = sign_in(&harness).await;

    let mut throttled = None;
    for attempt in 0..12 {
        let (status, body) = post_json(
            &harness.router,
            "/api/v1/auth/login",
            serde_json::json!({ "password": "wrong" }),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            throttled = Some((attempt, body));
            break;
        }
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt}: {body}"
        );
    }
    let (attempt, body) = throttled.expect("guessing was never throttled");
    assert!(attempt >= 5, "throttled after only {attempt} attempts");
    assert_eq!(body["code"], "auth.too_many_attempts", "{body}");
    assert!(
        body["params"]["seconds"].is_string(),
        "the refusal does not say how long to wait: {body}"
    );

    // The right password still gets a rejection while the lockout stands — the limiter runs
    // before the password is looked at, so a locked-out address learns nothing by guessing
    // correctly.
    let (status, _) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// The session list is credentials, not status: a monitoring token must not enumerate it.
#[tokio::test]
async fn a_read_only_token_cannot_see_the_session_inventory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, body) =
        common::get_with_bearer(&harness.router, "/api/v1/sessions", common::READ_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["params"]["scope"], "api:secrets", "{body}");
}

/// Nothing about a session may be readable without one.
#[tokio::test]
async fn the_session_inventory_needs_a_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, _) = get_json(&harness.router, "/api/v1/sessions").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Moves a session's sign-in and last use into the past (RD-130-09).
///
/// No API writes a past time and a test cannot wait hours for a limit to pass, so the row is
/// aged directly — the way `torrent.rs` ages a finished package.
async fn age(
    harness: &common::Harness,
    token: &str,
    signed_in_hours_ago: i64,
    used_hours_ago: i64,
) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", harness.database_path.display()))
        .await
        .expect("pool");
    let now = chrono::Utc::now();
    let changed =
        sqlx::query("UPDATE sessions SET created_at = ?, last_used_at = ? WHERE token_sha256 = ?")
            .bind(now - chrono::Duration::hours(signed_in_hours_ago))
            .bind(now - chrono::Duration::hours(used_hours_ago))
            .bind(hex::encode(Sha256::digest(token.as_bytes())))
            .execute(&pool)
            .await
            .expect("age the session");
    assert_eq!(changed.rows_affected(), 1, "no such session");
    pool.close().await;
}

/// Saves both session limits through the settings route, as `admin`.
async fn set_limits(
    harness: &common::Harness,
    admin: &str,
    idle_hours: u32,
    max_hours: u32,
) -> (StatusCode, serde_json::Value) {
    let (status, mut settings) =
        common::get_with_cookie(&harness.router, "/api/v1/settings", admin).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["session_idle_hours"] = serde_json::json!(idle_hours);
    settings["session_max_hours"] = serde_json::json!(max_hours);
    common::put_json_with_cookie(&harness.router, "/api/v1/settings", admin, settings).await
}

/// Whether `token` still authenticates, and the refusal's code when it does not.
async fn still_signed_in(harness: &common::Harness, token: &str) -> Result<(), String> {
    let (status, body) = common::get_with_cookie(&harness.router, "/api/v1/settings", token).await;
    match status {
        StatusCode::OK => Ok(()),
        _ => Err(format!("{status} {}", body["code"])),
    }
}

/// The limits are checked by the service, not only by the form: a value outside either range
/// is refused with a code that names which one and the range it has to be in.
#[tokio::test]
async fn the_session_limits_are_bounded_by_the_service() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let admin = sign_in(&harness).await;

    for (idle, max, code, min_allowed, max_allowed) in [
        (0, 720, "settings.session_idle_invalid", "1", "720"),
        (721, 720, "settings.session_idle_invalid", "1", "720"),
        (12, 0, "settings.session_max_invalid", "1", "2160"),
        (12, 2161, "settings.session_max_invalid", "1", "2160"),
    ] {
        let (status, body) = set_limits(&harness, &admin, idle, max).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{idle}/{max}: {body}");
        assert_eq!(body["code"], code, "{idle}/{max}: {body}");
        assert_eq!(body["params"]["min"], min_allowed, "{body}");
        assert_eq!(body["params"]["max"], max_allowed, "{body}");
    }

    // Both ends of both ranges are allowed, and what was saved is what comes back.
    let (status, body) = set_limits(&harness, &admin, 720, 2160).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session_idle_hours"], 720);
    assert_eq!(body["session_max_hours"], 2160);
    let (status, body) = set_limits(&harness, &admin, 1, 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The idle limit slides with use, and lowering it ends a session that has been idle longer
/// than the new value on its next request — not at its next sign-in (RD-130-09).
#[tokio::test]
async fn a_session_idle_for_longer_than_the_idle_limit_ends() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let admin = sign_in(&harness).await;
    let idle = sign_in(&harness).await;

    // Unused for three hours, under a four-hour idle limit: still signed in, and the request
    // that says so is itself a use.
    age(&harness, &idle, 10, 3).await;
    let (status, body) = set_limits(&harness, &admin, 4, 720).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(still_signed_in(&harness, &idle).await, Ok(()));

    // Unused for three hours again, and now the limit is two.
    age(&harness, &idle, 10, 3).await;
    let (status, body) = set_limits(&harness, &admin, 2, 720).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        still_signed_in(&harness, &idle).await,
        Err("401 Unauthorized \"auth.session_required\"".to_owned()),
        "a session idle past the limit still authenticated"
    );
    // The session that saved the setting has been in use all along.
    assert_eq!(still_signed_in(&harness, &admin).await, Ok(()));
}

/// The maximum lifetime ends a session however busy it is, and lowering it ends the sessions
/// already past the new value at once — otherwise lowering it after a lost laptop would
/// protect nothing until that laptop's session ran out on its own (RD-130-09).
#[tokio::test]
async fn a_shorter_maximum_ends_existing_sessions_at_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let admin = sign_in(&harness).await;
    let older = sign_in(&harness).await;

    // Signed in five hours ago and in use right now.
    age(&harness, &older, 5, 0).await;
    let (status, body) = set_limits(&harness, &admin, 12, 6).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(still_signed_in(&harness, &older).await, Ok(()));

    let (status, body) = set_limits(&harness, &admin, 12, 4).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        still_signed_in(&harness, &older).await,
        Err("401 Unauthorized \"auth.session_required\"".to_owned()),
        "a shorter maximum left an older session signed in"
    );
    assert_eq!(still_signed_in(&harness, &admin).await, Ok(()));

    // The inventory agrees: only the session that is still inside the maximum is listed.
    let (status, body) = common::get_with_cookie(&harness.router, "/api/v1/sessions", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
}

/// The browser keeps the cookie exactly as long as the maximum lifetime in force at sign-in.
#[tokio::test]
async fn the_cookie_lives_as_long_as_the_maximum_lifetime() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let admin = sign_in(&harness).await;

    let cookie = common::login_cookie(&harness.router, PASSWORD).await;
    assert!(
        cookie.contains("; Max-Age=2592000"),
        "thirty days by default: {cookie}"
    );

    let (status, body) = set_limits(&harness, &admin, 12, 48).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cookie = common::login_cookie(&harness.router, PASSWORD).await;
    assert!(cookie.contains("; Max-Age=172800"), "{cookie}");
    // One attribute per `;`: the old format string carried a run of spaces into the header.
    assert!(!cookie.contains("  "), "{cookie}");
}

/// A changed limit is in the audit log under its own name.
#[tokio::test]
async fn a_changed_session_limit_is_audited() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let admin = sign_in(&harness).await;

    let (status, body) = set_limits(&harness, &admin, 8, 168).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    let changed = records
        .iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .find(|record| record["action"] == "settings_changed")
        .expect("no settings_changed record");
    let fields = changed["details"]["fields"].as_str().expect("fields");
    assert!(fields.contains("session_idle_hours"), "{changed}");
    assert!(fields.contains("session_max_hours"), "{changed}");
}
