//! The optional second factor: enrolling it, being asked for it, and getting back in without it.
//!
//! The last of those is the one that matters most for a self-hosted service. There is no
//! support desk to appeal to, so every path here is shaped around "the phone is gone" rather
//! than around making the factor as strict as it could be.

mod common;

use axum::http::StatusCode;
use common::{auth_harness, get_with_cookie, post_json, post_json_with_cookie};

const PASSWORD: &str = "correct-horse-battery";

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
    let (_, _, token) = common::post_json_with_headers(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    token.expect("a session")
}

/// Enrols a factor and returns `(credential id, base32 secret, recovery codes)`.
async fn enrol(harness: &common::Harness, token: &str) -> (String, String, Vec<String>) {
    let (status, body) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/totp",
        token,
        serde_json::json!({ "label": "Phone" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let codes = body["recovery_codes"]
        .as_array()
        .expect("recovery codes")
        .iter()
        .map(|value| value.as_str().expect("a code").to_owned())
        .collect();
    (
        body["credential_id"].as_str().expect("id").to_owned(),
        body["secret"].as_str().expect("secret").to_owned(),
        codes,
    )
}

/// The inverse of the encoder, so a test can produce the code an app would show.
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

/// An enrolment that was never confirmed must not gate sign-in.
///
/// Somebody who scans a QR code badly, or scans it into an app they then delete, would
/// otherwise be locked out by the act of trying to be safer.
#[tokio::test]
async fn an_unconfirmed_enrolment_does_not_lock_anyone_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let _ = enrol(&harness, &token).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_confirmed_factor_is_required_at_the_next_sign_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, _) = enrol(&harness, &token).await;

    let (status, body) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The password alone is now refused, and says why.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "auth.mfa_required", "{body}");

    // With the code it goes through.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": current_code(&secret) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A wrong password must not be told that the account uses a second factor.
///
/// `auth.mfa_required` is only correct once the password was right; sent earlier it would
/// answer a question an unauthenticated caller has not earned.
#[tokio::test]
async fn a_wrong_password_never_reveals_that_a_second_factor_exists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, _) = enrol(&harness, &token).await;
    let (_, _) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body["code"], "auth.invalid_credentials",
        "a wrong password disclosed the second factor: {body}"
    );
}

/// The phone is gone. This is the path that has to work.
#[tokio::test]
async fn a_recovery_code_signs_you_in_and_then_stops_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, codes) = enrol(&harness, &token).await;
    let (_, _) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": codes[0] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a recovery code did not work: {body}"
    );

    // Single use: a printout left in a drawer is not a permanent bypass.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": codes[0] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a spent recovery code still worked: {body}"
    );

    // …and the others still do.
    let (status, _) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": codes[1] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Switching the factor off takes the password, not a code — a lost phone must not be final.
#[tokio::test]
async fn the_factor_is_switched_off_with_the_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, _) = enrol(&harness, &token).await;
    let (_, _) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;

    let (status, body) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/disable",
        &token,
        serde_json::json!({ "password": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let (status, body) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/disable",
        &token,
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, _) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the factor was not really switched off"
    );
}

/// The status page says how many recovery codes are left, because running out only becomes
/// visible at the worst possible moment.
#[tokio::test]
async fn the_status_reports_the_factor_and_the_remaining_codes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;

    let (status, body) = get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false, "{body}");
    assert_eq!(body["recovery_codes_remaining"], 0, "{body}");

    let (id, secret, _) = enrol(&harness, &token).await;
    let (_, _) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;

    let (_, body) = get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    assert_eq!(body["enabled"], true, "{body}");
    assert_eq!(body["recovery_codes_remaining"], 10, "{body}");
    assert_eq!(body["credentials"][0]["label"], "Phone", "{body}");
    // The seed must not travel with the inventory.
    assert!(
        !body.to_string().contains(&secret),
        "the status echoed the shared secret back"
    );
}

/// The seed must not leave in a settings backup.
///
/// The export is opt-in — it dereferences the references it is handed, resource by resource —
/// so the second factor is excluded today because nothing adds it. That is the right default
/// and a fragile one: adding a line to `settings_backup.rs` would put an authenticator seed
/// and ten recovery codes into a file people copy between machines and send to each other for
/// support. This test is what makes that line fail.
#[tokio::test]
async fn a_settings_export_does_not_carry_the_second_factor() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (_, secret, codes) = enrol(&harness, &token).await;

    let (status, bundle) = post_json_with_cookie(
        &harness.router,
        "/api/v1/settings/export",
        &token,
        serde_json::json!({ "include_secrets": true, "passphrase": "a-long-passphrase" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    let serialised = bundle.to_string();
    assert!(
        !serialised.contains(&secret),
        "the settings export carried the authenticator seed"
    );
    for code in &codes {
        assert!(
            !serialised.contains(code),
            "the settings export carried a recovery code"
        );
    }
}

/// A code is one-time, not one-window.
///
/// TOTP accepts a step either side of now, so a code that is merely noted as "used" stays
/// valid for about ninety seconds. That is long enough to replay one read over a shoulder,
/// captured by a phishing proxy or left in a client log — and being one-time is the whole
/// reason a one-time password exists.
#[tokio::test]
async fn an_accepted_totp_code_cannot_be_replayed_inside_its_window() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, _) = enrol(&harness, &token).await;
    let (status, body) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let code = current_code(&secret);
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": code }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the code did not work at all: {body}"
    );

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": code }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the same six digits were accepted twice: {body}"
    );
    assert_eq!(
        body["code"], "auth.invalid_credentials",
        "a replayed code must be indistinguishable from a wrong one: {body}"
    );

    // The factor still works. Refusing a replay must not lock the owner out of the next code.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": current_code(&secret) }),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::UNAUTHORIZED,
        "{body}"
    );
}

/// Somebody who finds the printout but not the password must not be able to burn the sheet.
///
/// The login used to consult and *spend* the second factor before the password had decided
/// anything, so an unauthenticated caller could replay a recovery code against a login that
/// was refused anyway and permanently use it up — ten requests to strand the owner.
#[tokio::test]
async fn a_wrong_password_does_not_spend_a_recovery_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness).await;
    let (id, secret, codes) = enrol(&harness, &token).await;
    let (status, body) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &token,
        serde_json::json!({ "code": current_code(&secret) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": "wrong", "code": codes[0] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        body["code"], "auth.invalid_credentials",
        "the refusal has to be the same whichever half was wrong: {body}"
    );

    // The count the owner sees has not moved either.
    let (status, status_body) = get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    assert_eq!(status, StatusCode::OK, "{status_body}");
    assert_eq!(
        status_body["recovery_codes_remaining"],
        serde_json::json!(codes.len()),
        "a refused sign-in spent a recovery code: {status_body}"
    );

    // And it still works for the person who also knows the password.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": PASSWORD, "code": codes[0] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the recovery code had already been burned: {body}"
    );
}
