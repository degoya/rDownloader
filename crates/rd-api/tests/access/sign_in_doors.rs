//! Every door that takes the administrator password or opens a session, side by side
//! (RD-1120-19).
//!
//! `AuthService::gate` replaced six copies of "locked out: `429`, otherwise wait out the global
//! slow-down" (audit 1.9.1, API-10). The copies asked the limiter twice and spelled the refusal
//! out each on their own; with one gate a locked-out address meets the same refusal at every
//! door, and a door that loses the gate fails here. Beside it the password change, the one
//! door that hands out a session from inside the API: only to a caller that is a session.

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use serde_json::{Value, json};

use crate::common::{self, API_BEARER, auth_harness};

const PASSWORD: &str = "correct-horse-battery";
const WRONG: &str = "not-the-password-at-all";
const REPLACEMENT: &str = "a-quite-different-passphrase";

/// Locks the harness's address out with wrong sign-ins.
async fn lock_out(router: &axum::Router) {
    for _ in 0..20 {
        let (status, body) =
            common::post_json(router, "/api/v1/auth/login", json!({ "password": WRONG })).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            return;
        }
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
    panic!("twenty wrong sign-ins never locked the address out");
}

/// The gate's refusal: the coded `429` with the seconds to wait.
fn assert_locked(door: &str, status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{door}: {body}");
    assert_eq!(body["code"], "auth.too_many_attempts", "{door}: {body}");
    let seconds = body["params"]["seconds"]
        .as_str()
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .unwrap_or_default();
    assert!(seconds >= 1, "{door} names no wait: {body}");
}

#[tokio::test]
async fn a_locked_out_address_meets_the_same_refusal_at_every_door() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let router = &harness.router;
    let session = common::sign_in(router, PASSWORD).await;
    lock_out(router).await;

    // The right password opens none of them while the lockout lasts.
    let (status, body) = common::post_json(
        router,
        "/api/v1/auth/login",
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_locked("the sign-in", status, &body);
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/password",
        &session,
        json!({ "current_password": PASSWORD, "new_password": REPLACEMENT }),
    )
    .await;
    assert_locked("the password change", status, &body);
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/oidc/link",
        &session,
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_locked("the step-up", status, &body);
    let (status, body) =
        common::post_json(router, "/api/v1/auth/passkey/challenge", json!({})).await;
    assert_locked("the passkey challenge", status, &body);

    // The two browser and compatibility doors answer in their own form, still refused.
    let (status, headers, _) = common::send_raw(
        router,
        common::request_to("GET", "/api/v1/auth/oidc/start")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        location.contains("oidc_error=auth.too_many_attempts"),
        "the provider sign-in started anyway: {location}"
    );
    let (status, _, text) = common::send_text(
        router,
        common::request_to("POST", "/api/v2/auth/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("password={API_BEARER}")))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("banned"), "{text}");
}

/// A token that holds the credentials area and knows the password may change it, and is handed
/// no browser session for it: that session would have passed neither the second factor nor a
/// password form switched off for the identity provider.
#[tokio::test]
async fn a_password_change_by_a_token_hands_out_no_session() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let router = &harness.router;
    common::sign_in(router, PASSWORD).await;

    let (status, body, session) = common::send_with_cookie(
        router,
        common::request_to("POST", "/api/v1/auth/password")
            .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "current_password": PASSWORD, "new_password": REPLACEMENT }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "auth.password_changed");
    assert_eq!(session, None, "a bearer token was handed a browser session");
}
