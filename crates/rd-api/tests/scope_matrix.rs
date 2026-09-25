//! Every route, refused for every scope it does not require.
//!
//! The point of this file is that it states no expectations of its own. It walks
//! `rd_api::policy_rows()` — the same table the middleware consults — and for each route
//! mints a token holding **every scope except the one that route needs**, then asserts a 403.
//! A test that restated the classification would only prove that two hand-written lists agree
//! with each other; deriving it proves the table is actually enforced, whatever the table says.
//!
//! Exhaustive by construction, so a route added tomorrow is covered the moment it gets its
//! policy entry — and it cannot ship without one, because `scope_policy`'s own tests compare
//! the table against the OpenAPI document in both directions.
//!
//! One test binary, deliberately. Each `rd-api` integration binary links the whole dependency
//! graph, and AGENTS.md is explicit about what thirty of them do to this machine.

mod common;

use axum::http::StatusCode;
use common::{auth_harness, get_with_bearer, request_with_bearer};
use rd_core::Scope;
use sha2::{Digest, Sha256};

/// Replaces path parameters with a syntactically valid value.
///
/// A refused request never reaches a handler, so the id only has to route.
fn concrete_path(path: &str) -> String {
    let mut result = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let end = rest.find('}').expect("closing brace");
        result.push_str(&rest[..start]);
        result.push_str("00000000-0000-7000-8000-000000000000");
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

/// Mints a bearer holding exactly `scopes` and returns it.
async fn bearer_holding(database: &rd_db::Database, label: &str, scopes: &[&str]) -> String {
    let bearer = format!("scope-matrix-{label}");
    let result = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            label.to_owned(),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        )
        .await;
    if let Err(error) = result {
        assert!(
            error.to_string().contains("UNIQUE constraint failed"),
            "minting {label}: {error}"
        );
    }
    bearer
}

/// A token holding every scope but the required one is refused, on every route.
///
/// This is the negative half of the policy, and the half that actually matters: proving a
/// scope *grants* access proves nothing about whether anything is withheld.
#[tokio::test]
async fn a_token_missing_only_the_required_scope_is_refused_everywhere() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // One bearer per scope, each holding every scope that does **not** confer the missing
    // one. "All the others" would not do: `Config` confers `Read`, so a token holding
    // everything but `Read` can still read, and the assertion would fail against a policy
    // working exactly as designed. Filtering by `satisfies` asks the same question the
    // middleware does.
    //
    // Minted once rather than per route: there are 233 rows and each mint is a database write.
    let mut without = std::collections::HashMap::new();
    for missing in Scope::API {
        let held: Vec<&str> = Scope::API
            .iter()
            .filter(|scope| !scope.satisfies(*missing))
            .map(|scope| scope.as_str())
            .collect();
        let bearer = bearer_holding(
            &harness.database,
            &format!("without-{}", missing.as_str().replace(':', "-")),
            &held,
        )
        .await;
        without.insert(missing.as_str(), bearer);
    }

    let mut checked = 0_usize;
    for (path, method, required) in rd_api::policy_rows() {
        let Some(required) = required else { continue };
        // The capture surface has its own layer and its own credential class; a token minted
        // here never reaches it, so there is nothing for this matrix to say about it.
        if required == rd_core::CAPTURE_SCOPE {
            continue;
        }
        let bearer = without.get(required).expect("a bearer for every scope");
        let (status, body) =
            request_with_bearer(&harness.router, method, &concrete_path(path), bearer).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} requires {required} but answered {status} to a token holding \
             every other scope: {body}"
        );
        assert_eq!(body["code"], "auth.scope_insufficient", "{method} {path}");
        assert_eq!(body["params"]["scope"], required, "{method} {path}");
        checked += 1;
    }
    assert!(checked > 200, "only {checked} routes were checked");
}

// The positive direction — "a token holding the required scope gets through" — is
// deliberately *not* driven through the router here. Doing so reaches real handlers: an
// account test opens a network connection, a subscription poll contacts an indexer, a tool
// probe spawns eight binaries with a five-second timeout each. A first attempt at it ran for
// ten minutes before being killed, and a suite that cannot finish proves nothing.
//
// That direction is decided rather than executed instead: `auth::tests` exercises
// `scope_refusal` against this same table, and every functional test in this crate already
// runs with a token that holds what it needs.

/// The pre-authentication routes stay reachable with no credential at all.
///
/// A route that quietly became public is the regression this catches, and it is the one the
/// negative matrix above cannot see: it only ever looks at routes that require something.
#[tokio::test]
async fn the_public_routes_need_no_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let mut checked = 0_usize;
    for (path, method, required) in rd_api::policy_rows() {
        if required.is_some() {
            continue;
        }
        let (status, body) =
            request_with_bearer(&harness.router, method, &concrete_path(path), "").await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} is classified public but demands a credential: {body}"
        );
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} is classified public but was refused: {body}"
        );
        checked += 1;
    }
    // health, openapi.json, auth status/setup/login/logout, and the two passkey sign-in
    // halves. Pinned as a number so *adding* a public route is a deliberate act: the
    // loop above can only check the routes it is given, and a new one arriving unnoticed
    // is exactly the regression worth catching.
    assert_eq!(checked, 8, "the set of public routes changed");
}

/// Clearing the notification history costs `api:admin`, like the other clears (RD-130-08).
///
/// The matrix above proves the table is enforced whatever it says; this pins what it says for
/// the one route where the obvious guess is wrong. The history is read with `api:config`, so a
/// table entry copied from its neighbour would let every configuration token throw it away.
#[tokio::test]
async fn a_config_token_reads_the_notification_history_but_cannot_clear_it() {
    const CLEAR: &str = "/api/v1/notifications/deliveries/clear";
    let required = rd_api::policy_rows()
        .into_iter()
        .find(|(path, method, _)| *path == CLEAR && *method == "POST")
        .and_then(|(_, _, required)| required);
    assert_eq!(required, Some(Scope::Admin.as_str()));

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(
        &harness.database,
        "notifications-config",
        &[Scope::Read.as_str(), Scope::Config.as_str()],
    )
    .await;

    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/notifications/deliveries", &bearer).await;
    assert_eq!(status, StatusCode::OK, "reading the history: {body}");

    let (status, body) = request_with_bearer(&harness.router, "POST", CLEAR, &bearer).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    assert_eq!(body["params"]["scope"], Scope::Admin.as_str(), "{body}");
}

/// The settings blob is priced `api:config`, but a few of its fields are not configuration.
///
/// `admin_login_disabled` is the sharp one: switching it on makes `granted_scopes` hand the
/// full `Scope::API` to every caller, credential or not. A token holding only `api:config`
/// could therefore mint itself `api:secrets` and `api:admin` through a route the policy table
/// says costs `api:config` — the table was right about the route and wrong about the payload.
#[tokio::test]
async fn a_config_token_cannot_switch_off_the_administrator_login() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(&harness.database, "config-only", &[Scope::Config.as_str()]).await;

    let (status, settings) = get_with_bearer(&harness.router, "/api/v1/settings", &bearer).await;
    assert_eq!(status, StatusCode::OK, "reading settings costs api:config");

    // Unchanged apart from the one field: this must be refused for what it changes, not for
    // being malformed.
    let mut escalated = settings.clone();
    escalated["admin_login_disabled"] = serde_json::Value::Bool(true);
    let (status, body) = put_settings(&harness.router, &bearer, &escalated).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient");
    assert_eq!(body["params"]["setting"], "admin_login_disabled");

    // The same token still writes an ordinary field, or the gate would have swallowed the
    // scope it is supposed to leave alone.
    let mut ordinary = settings.clone();
    ordinary["max_active_files"] = serde_json::json!(3);
    let (status, body) = put_settings(&harness.router, &bearer, &ordinary).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["max_active_files"], 3);
}

/// Lengthening how long a sign-in lasts widens what a stolen cookie is worth, so it costs the
/// administration scope like the other fields that decide who may reach the service
/// (RD-130-09).
#[tokio::test]
async fn a_config_token_cannot_change_how_long_a_sign_in_lasts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(&harness.database, "config-only", &[Scope::Config.as_str()]).await;

    let (status, settings) = get_with_bearer(&harness.router, "/api/v1/settings", &bearer).await;
    assert_eq!(status, StatusCode::OK, "reading settings costs api:config");

    for (field, value) in [("session_idle_hours", 720), ("session_max_hours", 2160)] {
        let mut longer = settings.clone();
        longer[field] = serde_json::json!(value);
        let (status, body) = put_settings(&harness.router, &bearer, &longer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{field}: {body}");
        assert_eq!(body["code"], "auth.scope_insufficient");
        assert_eq!(body["params"]["setting"], field);
    }
}

async fn put_settings(
    router: &axum::Router,
    bearer: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/settings")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {bearer}"),
        )
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request");
    common::send(router, request).await
}
