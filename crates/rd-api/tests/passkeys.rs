//! Passkeys: enrolling one, signing in with it, and the ways it must refuse.
//!
//! Driven with a software authenticator rather than a mock, so what these exercise is the real
//! ceremony — a challenge this service issued, a signature over it, and the origin binding that
//! is the whole reason the signature means anything. A hand-written stand-in would agree with
//! whatever the handler happened to do, which is precisely the property a broken handler and a
//! broken stand-in would share.
//!
//! Separate from `mfa.rs` because a passkey is not the same feature as the authenticator app:
//! one is another way to take the first step, the other is a second step. Several of the tests
//! below exist to hold that line.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use common::auth_harness;
use http_body_util::BodyExt;
use tower::ServiceExt;
use webauthn_authenticator_rs::{AuthenticatorBackend, softtoken::SoftToken};

const PASSWORD: &str = "correct-horse-battery";

/// Where the harness's requests come from. Loopback, so it is usable without an external URL
/// being configured — which is the state a fresh install is in, and the one most people stay in.
const ORIGIN: &str = "http://localhost:8710";

/// One request, decoded, with the `Set-Cookie` kept: a sign-in is only proven by the session.
async fn send(
    router: &Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("rd_session="))
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        cookie,
    )
}

/// A POST carrying the browser's `Origin`, which is what binds a ceremony to this service.
fn from_origin(uri: &str, token: Option<&str>, body: &serde_json::Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "localhost:8710")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::COOKIE, format!("rd_session={token}"));
    }
    request.body(Body::from(body.to_string())).expect("request")
}

async fn sign_in(router: &Router) -> String {
    let (status, body, _) = send(
        router,
        from_origin(
            "/api/v1/auth/setup",
            None,
            &serde_json::json!({ "password": PASSWORD }),
        ),
    )
    .await;
    assert!(
        status.is_success() || body["code"] == "auth.setup_completed",
        "setup: {body}"
    );
    let (status, body, token) = send(
        router,
        from_origin(
            "/api/v1/auth/login",
            None,
            &serde_json::json!({ "password": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    token.expect("a session")
}

/// Runs the whole enrolment ceremony, returning the authenticator that now holds the key —
/// a passkey is only useful to the device that made it.
async fn enrol(router: &Router, token: &str, label: &str) -> SoftToken {
    let (mut authenticator, _) = SoftToken::new(true).expect("a software authenticator");
    let (status, challenge, _) = send(
        router,
        from_origin("/api/v1/mfa/passkey", Some(token), &serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{challenge}");

    let options = serde_json::from_value(challenge["options"]["publicKey"].clone())
        .expect("creation options");
    let credential = authenticator
        .perform_register(ORIGIN.parse().expect("origin"), options, 10_000)
        .expect("the authenticator produced a credential");

    let (status, body, _) = send(
        router,
        from_origin(
            "/api/v1/mfa/passkey/confirm",
            Some(token),
            &serde_json::json!({
                "ceremony_id": challenge["ceremony_id"],
                "label": label,
                "credential": serde_json::to_value(credential).expect("credential"),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "webauthn");
    assert_eq!(body["label"], label);
    assert!(
        body["confirmed_at"].is_string(),
        "the ceremony is itself the proof the key works, so it is confirmed on arrival: {body}"
    );
    authenticator
}

/// Signs in with a passkey, returning the session cookie or the refusal.
async fn passkey_sign_in(
    router: &Router,
    authenticator: &mut SoftToken,
) -> Result<String, (StatusCode, serde_json::Value)> {
    let (status, challenge, _) = send(
        router,
        from_origin(
            "/api/v1/auth/passkey/challenge",
            None,
            &serde_json::json!({}),
        ),
    )
    .await;
    if status != StatusCode::OK {
        return Err((status, challenge));
    }
    let options =
        serde_json::from_value(challenge["options"]["publicKey"].clone()).expect("request options");
    let assertion = authenticator
        .perform_auth(ORIGIN.parse().expect("origin"), options, 10_000)
        .expect("the authenticator signed the challenge");
    let (status, body, cookie) = send(
        router,
        from_origin(
            "/api/v1/auth/passkey/login",
            None,
            &serde_json::json!({
                "ceremony_id": challenge["ceremony_id"],
                "credential": serde_json::to_value(assertion).expect("assertion"),
            }),
        ),
    )
    .await;
    match cookie {
        Some(token) if status == StatusCode::OK => Ok(token),
        _ => Err((status, body)),
    }
}

#[tokio::test]
async fn a_passkey_signs_in_without_the_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    let mut authenticator = enrol(&harness.router, &token, "Laptop").await;

    let session = passkey_sign_in(&harness.router, &mut authenticator)
        .await
        .expect("the passkey opened a session");
    // A real session, not merely a cookie: it reaches a route that requires one.
    let (status, body) = common::get_with_cookie(&harness.router, "/api/v1/mfa", &session).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The regression the whole design turns on. Enrolling a passkey must not make the password
/// login start demanding a code from an authenticator app that does not exist — that would
/// lock the owner out by the act of adding a second way in.
#[tokio::test]
async fn enrolling_a_passkey_does_not_turn_on_the_code_prompt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    enrol(&harness.router, &token, "Laptop").await;

    let (status, body, cookie) = send(
        &harness.router,
        from_origin(
            "/api/v1/auth/login",
            None,
            &serde_json::json!({ "password": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the password alone still signs in: {body}"
    );
    assert!(cookie.is_some());

    let (_, status_body) = common::get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    assert_eq!(
        status_body["enabled"], false,
        "a passkey is a way in, not a second step: {status_body}"
    );
}

/// A challenge is answerable once. Replaying a captured assertion must not open a second
/// session — which is what the ceremony store removing rather than reading is for.
#[tokio::test]
async fn an_assertion_cannot_be_replayed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    let mut authenticator = enrol(&harness.router, &token, "Laptop").await;

    let (status, challenge, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/auth/passkey/challenge",
            None,
            &serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{challenge}");
    let options =
        serde_json::from_value(challenge["options"]["publicKey"].clone()).expect("request options");
    let assertion = authenticator
        .perform_auth(ORIGIN.parse().expect("origin"), options, 10_000)
        .expect("a signed assertion");
    let payload = serde_json::json!({
        "ceremony_id": challenge["ceremony_id"],
        "credential": serde_json::to_value(assertion).expect("assertion"),
    });

    let (first, body, _) = send(
        &harness.router,
        from_origin("/api/v1/auth/passkey/login", None, &payload),
    )
    .await;
    assert_eq!(first, StatusCode::OK, "{body}");
    let (second, body, cookie) = send(
        &harness.router,
        from_origin("/api/v1/auth/passkey/login", None, &payload),
    )
    .await;
    assert_eq!(second, StatusCode::UNAUTHORIZED, "{body}");
    assert!(cookie.is_none(), "a replayed assertion opened a session");
}

/// Revoking a passkey has to end its ability to sign in, not merely hide it from the list.
#[tokio::test]
async fn a_revoked_passkey_stops_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    let mut authenticator = enrol(&harness.router, &token, "Laptop").await;

    let (_, listing) = common::get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    let id = listing["credentials"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/mfa/credentials/{id}"))
        .header(header::HOST, "localhost:8710")
        .header(header::COOKIE, format!("rd_session={token}"))
        .body(Body::empty())
        .expect("request");
    let (status, body, _) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let error = passkey_sign_in(&harness.router, &mut authenticator)
        .await
        .expect_err("a revoked passkey must not sign in");
    assert_eq!(error.0, StatusCode::NOT_FOUND, "{:?}", error.1);
}

/// Switching the authenticator app off must not take the passkeys with it. They are separate
/// ways in, and a settings toggle that silently removes one is how a lockout happens.
#[tokio::test]
async fn turning_off_the_code_prompt_leaves_the_passkeys_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    let mut authenticator = enrol(&harness.router, &token, "Laptop").await;

    let (status, body, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/mfa/disable",
            Some(&token),
            &serde_json::json!({ "password": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(
        passkey_sign_in(&harness.router, &mut authenticator)
            .await
            .is_ok(),
        "disabling the authenticator app removed the passkey too"
    );
}

/// Before anything is enrolled there is nothing to challenge, and the sign-in screen is told
/// so rather than being given a button that cannot work.
#[tokio::test]
async fn without_a_passkey_the_sign_in_screen_is_not_offered_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (status, body, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/auth/passkey/challenge",
            None,
            &serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "mfa.passkey_none_enrolled");

    let (_, auth_status) = common::get_json(&harness.router, "/api/v1/auth/status").await;
    assert_eq!(auth_status["passkeys_available"], false);
}

/// With no external URL configured and a non-loopback `Origin`, there is no honest answer to
/// "what would this credential protect", so the request is refused with the reason rather than
/// bound to whatever the caller claimed. Binding it to the claim is the phishing hole.
#[tokio::test]
async fn an_unusable_origin_is_refused_with_an_explanation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/mfa/passkey")
        .header(header::HOST, "downloads.example.com")
        .header(header::ORIGIN, "https://downloads.example.com")
        .header(header::COOKIE, format!("rd_session={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .expect("request");
    let (status, body, _) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "mfa.passkey_origin_unknown");
}

/// The seed of a second factor must not travel in a settings bundle, and neither must a
/// passkey. Both live in the same encrypted store as the credentials that *are* exported, so
/// the property holds only because the export is opt-in per resource — which is exactly the
/// kind of invariant that a later convenience change quietly breaks.
#[tokio::test]
async fn a_settings_export_does_not_carry_a_passkey() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    enrol(&harness.router, &token, "Laptop").await;

    let (_, listing) = common::get_with_cookie(&harness.router, "/api/v1/mfa", &token).await;
    let label = listing["credentials"][0]["label"]
        .as_str()
        .expect("a label");

    let (status, bundle, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/settings/export",
            Some(&token),
            &serde_json::json!({ "include_secrets": true, "passphrase": PASSWORD }),
        ),
    )
    .await;
    assert!(status.is_success(), "{bundle}");
    let serialised = bundle.to_string();
    assert!(
        !serialised.contains(label),
        "the passkey turned up in a settings bundle"
    );
    assert!(
        !serialised.contains("webauthn"),
        "the settings bundle mentions passkey material"
    );
}

/// The challenge endpoint is public, so it has to be metered.
///
/// It allocates in-flight server state for an anonymous caller, and it used to do so however
/// often it was asked. An address the limiter has already locked out for guessing passwords
/// must not be able to keep starting ceremonies while it waits out the lockout.
#[tokio::test]
async fn the_challenge_endpoint_is_refused_to_a_locked_out_address() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router).await;
    enrol(&harness.router, &token, "Laptop").await;

    // A working challenge first, so what the loop changes is the throttle and not the setup.
    let (status, body, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/auth/passkey/challenge",
            None,
            &serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut locked = false;
    for _ in 0..12 {
        let (status, _, _) = send(
            &harness.router,
            from_origin(
                "/api/v1/auth/login",
                None,
                &serde_json::json!({ "password": "wrong" }),
            ),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            locked = true;
            break;
        }
    }
    assert!(locked, "the password login was never throttled");

    let (status, body, _) = send(
        &harness.router,
        from_origin(
            "/api/v1/auth/passkey/challenge",
            None,
            &serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "a locked-out address could still start ceremonies: {body}"
    );
    assert_eq!(body["code"], "auth.too_many_attempts", "{body}");
}
