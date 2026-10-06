//! Signing in through an identity provider (RD-190-15, ADR 0021), end to end against a stand-in
//! provider (`common::idp`): discovery, key set and token endpoint, ID tokens signed with real
//! keys.
//!
//! The threats of the ADR that live in the HTTP flow each have a test here, named after the
//! threat: O-CSRF, O-STEAL, O-ISS, O-AUD, O-TIME, O-WHO, O-CONF, O-REDIR, O-JWKS and O-LOCK. The
//! rows that live in the token alone (O-ALG, O-REPLAY, the boundaries of O-TIME) are proven once
//! more, exhaustively, in `rd_authn::oidc_token`; O-FLOOD in `rd_authn::oidc`; O-LEAK in
//! `admin::canaries`.

mod browser;
mod review;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{StatusCode, header},
};
use browser::{
    ADMINISTRATOR, CALLBACK, PASSWORD, audited, callback, callback_with, configure, link,
    linked_world, location, parameter, provider_session, refusal, sign_in_as, start, start_raw,
    world_with,
};
use rd_core::AuditAction;
use serde_json::json;

use crate::common::{self, idp::CLIENT_ID};

const ADMIN_BEARER: &str = "oidc-admin-bearer";
const SECRETS_BEARER: &str = "oidc-secrets-bearer";
const CONTROL: &str = "the-local-control-token";

/// The whole way: configure, link by a round trip, sign in through the provider, and the session
/// that comes out is an ordinary one.
#[tokio::test]
async fn the_administrator_links_an_identity_and_signs_in_with_it() {
    let world = world_with(common::Options::default(), true).await;
    let router = &world.harness.router;
    let (_, status) = common::get_json(router, "/api/v1/auth/status").await;
    assert_eq!(status["oidc_available"], false, "{status}");
    assert_eq!(status["password_login"], true);

    let (code, settings) = configure(&world, json!({})).await;
    assert_eq!(code, StatusCode::OK, "{settings}");
    assert_eq!(settings["redirect_uri"], CALLBACK);
    assert_eq!(settings["client_secret_set"], true);
    assert!(
        settings.get("client_secret").is_none(),
        "the secret is write-only"
    );
    // Configured but nobody linked: nothing to offer on the sign-in screen yet.
    let (_, status) = common::get_json(router, "/api/v1/auth/status").await;
    assert_eq!(status["oidc_available"], false);

    let (code, headers) = link(&world, ADMINISTRATOR, 1).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/settings/security?oidc=linked");
    let linked = audited(&world, AuditAction::IdentityLinked).await;
    assert_eq!(linked.len(), 1);
    assert_eq!(
        linked[0].target_id.as_deref(),
        Some(world.idp.issuer.as_str())
    );
    let (_, status) = common::get_json(router, "/api/v1/auth/status").await;
    assert_eq!(status["oidc_available"], true);
    assert_eq!(status["oidc_display_name"], "Pocket ID");

    let started = start(&world, Some("/downloads?view=packages"), 2).await;
    assert_eq!(
        parameter(&started.authorization, "redirect_uri").as_deref(),
        Some(CALLBACK)
    );
    world.idp.expect_code(
        "sign-in-code",
        common::idp::Grant::for_request(&started.authorization, ADMINISTRATOR),
    );
    let (code, headers) = callback(
        &world,
        &started.state,
        "sign-in-code",
        Some(&started.binding),
        2,
    )
    .await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/downloads?view=packages");
    let session = common::session_cookie(&headers).expect("a session cookie");
    assert!(
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.starts_with("rd_oidc=;") && value.contains("Max-Age=0")),
        "the binding is cleared"
    );

    let (code, settings) = common::get_with_cookie(router, "/api/v1/auth/oidc", &session).await;
    assert_eq!(code, StatusCode::OK, "{settings}");
    assert_eq!(settings["provider_session"], true);
    assert_eq!(settings["identity"]["label"], "owner");
    let signed_in = audited(&world, AuditAction::LoginSucceeded).await;
    assert_eq!(
        signed_in[0].details.get("method").map(String::as_str),
        Some("oidc")
    );
    // One code, one redemption each, and the provider checked PKCE and the client secret.
    assert_eq!(world.idp.token_requests(), 2);
}

/// Without a provider the sign-in is unchanged, and the start says so instead of failing.
#[tokio::test]
async fn without_a_provider_nothing_changes() {
    let world = world_with(common::Options::default(), true).await;
    let (status, headers) = start_raw(&world, None, 3, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_not_configured")
    );
    let (status, _) = common::post_json(
        &world.harness.router,
        "/api/v1/auth/login",
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// The redirect URI is the external URL's; without one the provider cannot be switched on.
#[tokio::test]
async fn the_provider_needs_the_external_url() {
    let world = world_with(common::Options::default(), false).await;
    let (status, body) = configure(&world, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "auth.oidc_requires_external_url");
}

/// O-CSRF: a callback without the binding of the browser that started the flow, or with another
/// browser's, is refused — a stranger's page cannot sign somebody in with the stranger's code.
#[tokio::test]
async fn o_csrf_a_callback_without_the_starting_browsers_binding_is_refused() {
    let world = linked_world().await;
    let victim = start(&world, None, 10).await;
    world.idp.expect_code(
        "stranger-code",
        common::idp::Grant::for_request(&victim.authorization, ADMINISTRATOR),
    );
    let (status, headers) = callback(&world, &victim.state, "stranger-code", None, 11).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_state_invalid")
    );
    assert!(common::session_cookie(&headers).is_none());

    let victim = start(&world, None, 12).await;
    let other = start(&world, None, 13).await;
    let (_, headers) = callback(
        &world,
        &victim.state,
        "stranger-code",
        Some(&other.binding),
        12,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_state_invalid")
    );
    assert_eq!(
        world.idp.token_requests(),
        1,
        "only the link redeemed a code"
    );
    let refused = audited(&world, AuditAction::LoginFailed).await;
    assert!(
        refused
            .iter()
            .all(|record| record.details.get("stage").map(String::as_str) == Some("oidc_state")),
        "{refused:?}"
    );
}

/// O-STEAL: a callback URL replayed — from the history, a proxy log — finds its state spent.
#[tokio::test]
async fn o_steal_a_replayed_callback_is_refused() {
    let world = linked_world().await;
    let started = start(&world, None, 20).await;
    world.idp.expect_code(
        "once",
        common::idp::Grant::for_request(&started.authorization, ADMINISTRATOR),
    );
    let (status, headers) =
        callback(&world, &started.state, "once", Some(&started.binding), 20).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(common::session_cookie(&headers).is_some());
    let requests = world.idp.token_requests();
    // The same browser, the same URL: the state answered once.
    let (_, headers) = callback(&world, &started.state, "once", Some(&started.binding), 21).await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_state_invalid")
    );
    assert!(common::session_cookie(&headers).is_none());
    assert_eq!(world.idp.token_requests(), requests, "no second redemption");
}

/// O-ISS: a token from another issuer, an `iss` parameter naming another issuer (RFC 9207), and
/// a discovery document speaking for another issuer.
#[tokio::test]
async fn o_iss_another_issuer_is_refused_in_the_token_the_answer_and_the_discovery() {
    let world = linked_world().await;
    let (_, headers) = sign_in_as(
        &world,
        "wrong-issuer",
        |grant| grant.with_claims(json!({ "iss": "http://127.0.0.1:1" })),
        30,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_token_invalid")
    );

    let started = start(&world, None, 31).await;
    world.idp.expect_code(
        "mixed-up",
        common::idp::Grant::for_request(&started.authorization, ADMINISTRATOR),
    );
    let (_, headers) = callback_with(
        &world,
        &format!(
            "code=mixed-up&state={}&iss=http%3A%2F%2F127.0.0.1%3A1",
            started.state
        ),
        Some(&started.binding),
        31,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_issuer_mismatch")
    );

    let other = world_with(common::Options::default(), true).await;
    other
        .idp
        .override_discovery("issuer", json!("http://127.0.0.1:1"));
    let (status, body) = configure(&other, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "auth.oidc_issuer_mismatch");
}

/// O-AUD: a token minted for another client of the same provider.
#[tokio::test]
async fn o_aud_a_token_for_another_client_is_refused() {
    let world = linked_world().await;
    let (_, headers) = sign_in_as(
        &world,
        "foreign",
        |grant| grant.with_claims(json!({ "aud": "jellyfin" })),
        40,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_token_invalid")
    );
    let (_, headers) = sign_in_as(
        &world,
        "several",
        |grant| grant.with_claims(json!({ "aud": ["jellyfin", CLIENT_ID] })),
        41,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_token_invalid")
    );
}

/// O-TIME: a token expired beyond the minute of leeway.
#[tokio::test]
async fn o_time_an_expired_token_is_refused() {
    let world = linked_world().await;
    let expired = chrono::Utc::now().timestamp() - 61;
    let (_, headers) = sign_in_as(
        &world,
        "stale",
        |grant| grant.with_claims(json!({ "exp": expired })),
        50,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_token_invalid")
    );
}

/// O-WHO: any other account at the provider is refused — another subject with the same email,
/// the right group and the wrong subject, the right subject without the group.
#[tokio::test]
async fn o_who_only_the_bound_identity_is_the_administrator() {
    let world = linked_world().await;
    let (_, headers) = sign_in_as(
        &world,
        "impostor",
        |grant| {
            grant.with_claims(json!({
                "sub": "someone-else",
                "email": "owner@example.com",
                "preferred_username": "guest",
            }))
        },
        60,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_not_administrator")
    );
    // The person signed in with the wrong account can tell which one it was.
    assert_eq!(
        parameter(&location(&headers), "oidc_name").as_deref(),
        Some("guest")
    );

    let (status, body) = configure(
        &world,
        json!({ "group_claim": "groups", "group_value": "rd-admins" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["identity"].is_object(), "a group keeps the binding");
    let (_, headers) = sign_in_as(&world, "no-group", |grant| grant, 61).await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_not_administrator")
    );
    let (_, headers) = sign_in_as(
        &world,
        "group-only",
        |grant| grant.with_claims(json!({ "sub": "someone-else", "groups": ["rd-admins"] })),
        62,
    )
    .await;
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_not_administrator")
    );
    let (status, headers) = sign_in_as(
        &world,
        "both",
        |grant| grant.with_claims(json!({ "groups": ["family", "rd-admins"] })),
        63,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(common::session_cookie(&headers).is_some(), "{headers:?}");

    // Recorded by stage, never with the refused subject.
    let refused = audited(&world, AuditAction::LoginFailed).await;
    assert_eq!(
        refused
            .iter()
            .filter(|record| record.details.get("stage").map(String::as_str) == Some("oidc_account"))
            .count(),
        3
    );
    let everything = serde_json::to_string(&refused).expect("records");
    assert!(!everything.contains("someone-else"));
}

/// O-CONF: no token configures the provider, links an identity or switches the password sign-in
/// off — not one holding `api:admin`, not one holding `api:secrets`, not the full `api:*`.
#[tokio::test]
async fn o_conf_tokens_cannot_configure_link_or_switch_the_password_off() {
    let options = common::Options::default()
        .token(ADMIN_BEARER, "api:admin")
        .token(SECRETS_BEARER, "api:secrets");
    let world = world_with(options, true).await;
    let router = &world.harness.router;
    let body = json!({
        "password": PASSWORD,
        "issuer": world.idp.issuer,
        "client_id": CLIENT_ID,
        "client_secret": "a-secret-of-the-attackers-choosing",
        "display_name": "Evil",
    });
    for bearer in [ADMIN_BEARER, SECRETS_BEARER, common::API_BEARER] {
        for (method, uri) in [
            ("PUT", "/api/v1/auth/oidc"),
            ("DELETE", "/api/v1/auth/oidc"),
            ("POST", "/api/v1/auth/oidc/link"),
            ("DELETE", "/api/v1/auth/oidc/identity"),
            ("POST", "/api/v1/auth/password-login/off"),
        ] {
            let (status, answer) = common::send(
                router,
                common::request_to(method, uri)
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{bearer} {method} {uri}: {answer}"
            );
        }
    }
    let (_, settings) = common::get_with_cookie(router, "/api/v1/auth/oidc", &world.session).await;
    assert_eq!(settings["configured"], false, "nothing was configured");

    // A session without the password again is refused as well.
    let mut wrong = body;
    wrong["password"] = json!("not-the-password");
    let (status, answer) =
        common::put_json_with_cookie(router, "/api/v1/auth/oidc", &world.session, wrong).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
}

/// O-REDIR: the redirect URI comes from the external URL and from nothing a request says, and the
/// return path cannot leave the application.
#[tokio::test]
async fn o_redir_neither_a_header_nor_a_parameter_steers_the_redirects() {
    let world = linked_world().await;
    let (status, headers) = start_raw(
        &world,
        None,
        70,
        &[
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded-proto", "http"),
            ("forwarded", "host=evil.example;proto=http"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        parameter(&location(&headers), "redirect_uri").as_deref(),
        Some(CALLBACK)
    );

    for (peer, escape) in [(71, "//evil.example/x"), (72, "https://evil.example/")] {
        let started = start(&world, Some(escape), peer).await;
        world.idp.expect_code(
            "escape",
            common::idp::Grant::for_request(&started.authorization, ADMINISTRATOR),
        );
        let (status, headers) = callback(
            &world,
            &started.state,
            "escape",
            Some(&started.binding),
            peer,
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(location(&headers), "/", "{escape}");
    }
}

/// O-JWKS: a rotated key is fetched without a restart, and a burst of tokens naming unknown keys
/// costs the provider one fetch.
#[tokio::test]
async fn o_jwks_rotation_needs_no_restart_and_unknown_keys_cost_one_fetch() {
    let world = linked_world().await;
    assert_eq!(world.idp.jwks_fetches(), 1);
    provider_session(&world, "cached", 80).await;
    assert_eq!(world.idp.jwks_fetches(), 1, "the key set is cached");

    let rotated = world
        .idp
        .add_key(rd_authn::oidc_testing::TestKey::es256("key-2"), false);
    world.idp.publish(&[rotated]);
    let (status, headers) =
        sign_in_as(&world, "rotated", |grant| grant.signed_by(rotated), 81).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(common::session_cookie(&headers).is_some(), "{headers:?}");
    assert_eq!(world.idp.jwks_fetches(), 2);

    let forged = world
        .idp
        .add_key(rd_authn::oidc_testing::TestKey::es256("forged"), false);
    for (peer, code) in [(82, "forged-1"), (83, "forged-2"), (84, "forged-3")] {
        let (_, headers) = sign_in_as(&world, code, |grant| grant.signed_by(forged), peer).await;
        assert_eq!(
            refusal(&headers).as_deref(),
            Some("auth.oidc_token_invalid")
        );
    }
    assert_eq!(world.idp.jwks_fetches(), 2, "no fetch for a forged key id");
}

/// O-LOCK: the password form goes off only from a session the provider opened, and comes back
/// only from this machine — never through a session or a token, never from elsewhere.
#[tokio::test]
async fn o_lock_the_password_form_goes_off_from_a_provider_session_and_on_only_from_here() {
    let world = linked_world().await;
    let router = &world.harness.router;
    let off = json!({ "password": PASSWORD });
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/password-login/off",
        &world.session,
        off.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "auth.password_login_needs_provider_session");

    let provider = provider_session(&world, "proof", 90).await;
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/password-login/off",
        &provider,
        off.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = common::post_json(
        router,
        "/api/v1/auth/login",
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.password_login_off");
    let (_, status) = common::get_json(router, "/api/v1/auth/status").await;
    assert_eq!(status["password_login"], false);
    // Nothing that would end the provider sign-in while the password form is off.
    for (method, uri) in [
        ("DELETE", "/api/v1/auth/oidc"),
        ("DELETE", "/api/v1/auth/oidc/identity"),
    ] {
        let (status, body) = common::send(
            router,
            common::request_to(method, uri)
                .header(header::COOKIE, format!("rd_session={provider}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(off.to_string()))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}: {body}");
        assert_eq!(body["code"], "auth.oidc_password_login_off");
    }
    // The provider still signs in.
    provider_session(&world, "still", 91).await;

    // Back on: not from a session, not with a token, not from another machine.
    let control = world
        .harness
        .state
        .clone()
        .with_local_control(rd_api::local_control::LocalControl::for_token(CONTROL));
    let local = rd_api::router(control);
    let on = "/api/v1/auth/password-login/on";
    let (status, body) = common::post_json_with_cookie(&local, on, &provider, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.password_login_cli_only");
    let (status, body) = common::post_with_bearer(&local, on, common::API_BEARER, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.password_login_cli_only");
    let elsewhere = |peer: [u8; 4]| {
        common::request_to("POST", on)
            .header(header::AUTHORIZATION, format!("Bearer {CONTROL}"))
            .header(header::CONTENT_TYPE, "application/json")
            .extension(ConnectInfo(std::net::SocketAddr::from((peer, 50_000))))
            .body(Body::from("{}"))
            .expect("request")
    };
    let (status, _) = common::send(&local, elsewhere([192, 168, 1, 20])).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, status_body) = common::get_json(router, "/api/v1/auth/status").await;
    assert_eq!(status_body["password_login"], false);

    let (status, body) = common::send(&local, elsewhere([127, 0, 0, 1])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = common::post_json(
        router,
        "/api/v1/auth/login",
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let changes = audited(&world, AuditAction::PasswordLoginChanged).await;
    let enabled: Vec<Option<&str>> = changes
        .iter()
        .filter(|record| record.outcome == rd_core::AuditOutcome::Success)
        .map(|record| record.details.get("enabled").map(String::as_str))
        .collect();
    assert_eq!(enabled, [Some("true"), Some("false")], "newest first");
}

/// Cancelling at the provider is a choice: nothing is recorded and nothing counted.
#[tokio::test]
async fn a_sign_in_cancelled_at_the_provider_is_not_a_failure() {
    let world = linked_world().await;
    let started = start(&world, None, 100).await;
    let before = audited(&world, AuditAction::LoginFailed).await.len();
    let (_, headers) = callback_with(
        &world,
        &format!("error=access_denied&state={}", started.state),
        Some(&started.binding),
        100,
    )
    .await;
    assert_eq!(refusal(&headers).as_deref(), Some("auth.oidc_cancelled"));
    assert_eq!(
        audited(&world, AuditAction::LoginFailed).await.len(),
        before
    );
}

/// Signing out at the provider too is opt-in (D5); the local session ends either way.
#[tokio::test]
async fn signing_out_at_the_provider_is_opt_in() {
    let world = linked_world().await;
    let router = &world.harness.router;
    let session = provider_session(&world, "first", 110).await;
    let (status, body) =
        common::post_json_with_cookie(router, "/api/v1/auth/logout", &session, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("provider_logout_url").is_none(), "{body}");

    let (status, body) = configure(&world, json!({ "provider_logout": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session = provider_session(&world, "second", 111).await;
    let (_, body) =
        common::post_json_with_cookie(router, "/api/v1/auth/logout", &session, json!({})).await;
    let url = body["provider_logout_url"]
        .as_str()
        .expect("a provider logout URL");
    assert!(
        url.starts_with(&format!("{}/logout?", world.idp.issuer)),
        "{url}"
    );
    assert_eq!(parameter(url, "client_id").as_deref(), Some(CLIENT_ID));
    assert_eq!(
        parameter(url, "post_logout_redirect_uri").as_deref(),
        Some("https://dl.example.com/")
    );
    let (status, _) = common::get_with_cookie(router, "/api/v1/auth/oidc", &session).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the local session ended first"
    );
    let logouts = audited(&world, AuditAction::Logout).await;
    assert_eq!(
        logouts[0].details.get("provider").map(String::as_str),
        Some("true")
    );
    // Signed out by the session that ended, not by nobody.
    assert_eq!(logouts[0].actor_kind, rd_core::AuditActorKind::Session);
}

/// Changing the provider or the client ends the binding: `sub` means nothing elsewhere.
#[tokio::test]
async fn another_client_id_releases_the_binding() {
    let world = linked_world().await;
    // The stored secret was issued for the old client: another one needs its own.
    let (status, body) = configure(
        &world,
        json!({ "client_id": "another-client", "client_secret": null }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "auth.oidc_secret_required");
    // Keeping the provider keeps the secret.
    let (status, body) = configure(
        &world,
        json!({ "display_name": "Renamed", "client_secret": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["identity"].is_object(), "{body}");
    let (status, body) = configure(&world, json!({ "client_id": "another-client" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["identity"].is_null(), "{body}");
    assert_eq!(
        audited(&world, AuditAction::IdentityUnlinked).await.len(),
        1
    );
    let (_, status) = common::get_json(&world.harness.router, "/api/v1/auth/status").await;
    assert_eq!(status["oidc_available"], false);
    let (_, headers) = start_raw(&world, None, 120, &[]).await;
    assert_eq!(refusal(&headers).as_deref(), Some("auth.oidc_not_linked"));
}

/// The start is metered like every sign-in: a locked-out address cannot keep filing flows.
#[tokio::test]
async fn a_locked_out_address_cannot_start_a_flow() {
    let world = linked_world().await;
    // The limiter locks an address out after its sixth failure in a row.
    for _ in 0..6 {
        let (_, headers) = callback_with(&world, "code=x&state=unknown", None, 130).await;
        assert_eq!(
            refusal(&headers).as_deref(),
            Some("auth.oidc_state_invalid")
        );
    }
    let (status, headers) = start_raw(&world, None, 130, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(refusal(&headers).as_deref(), Some("auth.too_many_attempts"));
    // Another address is not affected.
    start(&world, None, 131).await;
}

/// A browser may send its cookies as several `Cookie` headers (HTTP/2); the binding is found in
/// any of them.
#[tokio::test]
async fn the_binding_is_found_in_any_cookie_header() {
    let world = linked_world().await;
    let started = start(&world, None, 140).await;
    world.idp.expect_code(
        "split",
        common::idp::Grant::for_request(&started.authorization, ADMINISTRATOR),
    );
    let (status, headers, _) = common::send_raw(
        &world.harness.router,
        browser::from_browser(
            "GET",
            &format!(
                "/api/v1/auth/oidc/callback?code=split&state={}",
                started.state
            ),
            140,
        )
        .header(header::COOKIE, "theme=dark")
        .header(header::COOKIE, format!("rd_oidc={}", started.binding))
        .body(Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(common::session_cookie(&headers).is_some(), "{headers:?}");
}
