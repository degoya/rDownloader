//! What the security review of the provider sign-in found (RD-1120-19), each with its test: a
//! link whose session ended on the way, and a group condition changed while the password form is
//! off. The rest of the review found nothing; the job file lists each point with its evidence.

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use serde_json::{Value, json};

use super::browser::{
    PASSWORD, callback, configure, cookie, from_browser, linked_world, parameter, provider_session,
    refusal, world_with,
};
use crate::common::{self, idp::Grant};

/// A link started from a session that signs out before the provider sends the browser back
/// binds nothing: the session and the password were the proof, and the session ended. Without
/// the check, a flow started by somebody who was then signed out — or locked out by a password
/// change — still bound their account ten minutes later.
#[tokio::test]
async fn a_link_whose_session_ended_on_the_way_binds_nothing() {
    let world = world_with(common::Options::default(), true).await;
    let router = &world.harness.router;
    let (status, body) = configure(&world, json!({})).await;
    assert_eq!(status, StatusCode::OK, "configure: {body}");

    let (status, headers, body) = common::send_raw(
        router,
        from_browser("POST", "/api/v1/auth/oidc/link", 120)
            .header(header::COOKIE, format!("rd_session={}", world.session))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "password": PASSWORD }).to_string()))
            .expect("request"),
    )
    .await;
    let body: Value = serde_json::from_slice(&body).expect("a JSON answer");
    assert_eq!(status, StatusCode::OK, "link: {body}");
    let authorization = body["authorization_url"]
        .as_str()
        .expect("an authorization URL")
        .to_owned();
    let binding = cookie(&headers, "rd_oidc").expect("the binding cookie");

    let (status, body) =
        common::post_json_with_cookie(router, "/api/v1/auth/logout", &world.session, json!({}))
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    world.idp.expect_code(
        "late-link",
        Grant::for_request(&authorization, "somebody-else"),
    );
    let (status, headers) = callback(
        &world,
        &parameter(&authorization, "state").expect("a state"),
        "late-link",
        Some(&binding),
        120,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        refusal(&headers).as_deref(),
        Some("auth.oidc_state_invalid")
    );
    assert_eq!(
        world.idp.token_requests(),
        0,
        "the code was redeemed for a link nobody stands behind any more"
    );
    let identity = world
        .harness
        .database
        .get_setting(rd_api::oidc_client::IDENTITY_SETTING)
        .await
        .expect("settings");
    assert!(
        identity.as_ref().is_none_or(|value| value.is_null()),
        "an identity was bound: {identity:?}"
    );
}

/// While the password form is off, a group condition the bound account may not meet would end
/// the provider sign-in like another provider would, and is refused like it; what does not
/// decide who signs in still changes.
#[tokio::test]
async fn a_group_condition_does_not_change_while_the_password_form_is_off() {
    let world = linked_world().await;
    let router = &world.harness.router;
    let provider = provider_session(&world, "proof", 121).await;
    let (status, body) = common::post_json_with_cookie(
        router,
        "/api/v1/auth/password-login/off",
        &provider,
        json!({ "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = configure(
        &world,
        json!({ "group_claim": "groups", "group_value": "rdownloader-admins" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "auth.oidc_password_login_off");

    let (status, body) = configure(&world, json!({ "display_name": "Authentik" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["display_name"], "Authentik");
    assert!(body["group_claim"].is_null(), "{body}");
    provider_session(&world, "after", 122).await;
}
