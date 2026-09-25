//! Scoped machine tokens: what an `api:read` bearer may reach, and what it may not.
//!
//! These run against a harness with the administrator login switched on. With the login
//! disabled every request is waved through before a scope is consulted, which is exactly
//! the case that would make these tests pass without the feature existing.

mod common;

use axum::http::StatusCode;
use common::{
    API_BEARER, CAPTURE_BEARER, READ_BEARER, auth_harness, get_json, get_with_bearer,
    post_with_bearer,
};

#[tokio::test]
async fn a_read_only_token_reads_the_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    for uri in [
        "/api/v1/downloads",
        "/api/v1/downloads/summary",
        "/api/v1/packages",
        "/api/v1/postprocess/queue",
        "/api/v1/storage/capacity",
        "/api/v1/system/media",
    ] {
        let (status, body) = get_with_bearer(&harness.router, uri, READ_BEARER).await;
        assert_eq!(status, StatusCode::OK, "{uri} answered {status}: {body}");
    }
}

#[tokio::test]
async fn a_read_only_token_is_refused_outside_its_allowlist() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // Configuration and intake are the two things this scope exists to keep out. A token
    // pasted into a status page must not enumerate categories, accounts or the settings
    // document, and must not be able to see what is being downloaded from where.
    for uri in [
        "/api/v1/settings",
        "/api/v1/categories",
        "/api/v1/accounts",
        "/api/v1/proxy-profiles",
        "/api/v1/collector/candidates",
        "/api/v1/auth-profiles",
        "/api/v1/api-tokens",
    ] {
        let (status, body) = get_with_bearer(&harness.router, uri, READ_BEARER).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} answered {status}: {body}"
        );
        assert_eq!(body["code"], "auth.scope_insufficient", "{uri}: {body}");
    }
}

#[tokio::test]
async fn a_read_only_token_cannot_change_anything() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // Same path as a permitted read, different method: the allowlist is method-scoped, so
    // this must not fall through to the GET entry.
    for (uri, body) in [
        (
            "/api/v1/downloads",
            serde_json::json!({ "url": "https://example.com/file.bin" }),
        ),
        (
            "/api/v1/downloads/bulk",
            serde_json::json!({ "ids": [], "action": "pause" }),
        ),
        (
            "/api/v1/collector/batches",
            serde_json::json!({ "text": "https://example.com/a" }),
        ),
    ] {
        let (status, response) = post_with_bearer(&harness.router, uri, READ_BEARER, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} answered {status}: {response}"
        );
        assert_eq!(
            response["code"], "auth.scope_insufficient",
            "{uri}: {response}"
        );
    }
}

#[tokio::test]
async fn a_full_api_token_also_reads_the_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // `api:*` satisfies `api:read`, so a machine client needs one token, not two.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_full_api_token_reaches_the_whole_surface() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // `api:*` means the whole API, the same surface a session reaches. The read allowlist
    // narrows `api:read`; it is not a ceiling on full access, or the command-line client and
    // every other machine client could only ever look.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/categories", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post_with_bearer(
        &harness.router,
        "/api/v1/downloads",
        API_BEARER,
        serde_json::json!({ "url": "https://example.com/file.bin" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn a_capture_token_reaches_no_queue_route() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // The scope strings share no implication: a browser extension token stays an intake
    // credential and never becomes a queue reader.
    //
    // A 403 rather than the 401 an anonymous request gets, and the difference is the point:
    // the bearer *is* a valid credential, it simply does not carry `api:read`. Answering 401
    // would tell the holder to go and authenticate, which they already have.
    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/downloads", CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    assert_eq!(body["params"]["scope"], "api:read", "{body}");

    // An anonymous request still gets 401: no credential at all is a different answer.
    let (anonymous_status, _) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(anonymous_status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_bearer_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn a_revoked_read_token_stops_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let tokens = harness
        .database
        .list_capture_tokens(&[rd_core::API_READ_SCOPE])
        .await
        .expect("tokens");
    let token = tokens.first().expect("seeded read token");
    harness
        .database
        .revoke_capture_token(token.id)
        .await
        .expect("revoke");

    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", READ_BEARER).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn a_read_only_token_reaches_the_mcp_transport() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    // The endpoint used to demand `api:*`, which made MCP all-or-nothing: an assistant that
    // should only watch the queue had to be given a token that could also read every stored
    // account. The transport now only establishes that a credential exists; which of the
    // sixteen tools it may call is decided per tool, where the tool name is known.
    // `crates/rd-api/tests/mcp.rs` covers that half.
    let (status, body) = post_with_bearer(
        &harness.router,
        "/mcp",
        READ_BEARER,
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
    )
    .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED, "{body}");

    // A capture token still does not: it carries no API scope at all.
    let (status, body) = post_with_bearer(
        &harness.router,
        "/mcp",
        CAPTURE_BEARER,
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "api.token_required", "{body}");
}

const ADMIN_PASSWORD: &str = "correct-horse-battery";

/// Sets the administrator password if this harness has none yet, then signs in.
async fn session(harness: &common::Harness) -> String {
    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": ADMIN_PASSWORD }),
    )
    .await;
    assert!(
        status.is_success() || body["code"] == "auth.setup_completed",
        "setup: {body}"
    );
    let (status, _, cookie) = common::post_json_with_headers(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": ADMIN_PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login");
    cookie.expect("a session cookie")
}

/// Minting a scoped token and then using it, end to end.
///
/// The scope matrix proves that *enforcement* matches the policy table, but it mints its
/// tokens directly in the database. This is the other half: what the REST call actually
/// stores, and whether a token minted through the API is then honoured as narrowly as it was
/// asked for. A widening bug in the minting handler would leave the matrix perfectly green.
#[tokio::test]
async fn a_token_minted_for_one_area_reaches_that_area_and_no_further() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let session = session(&harness).await;

    let (status, minted) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/api-tokens",
        &session,
        serde_json::json!({ "label": "Dashboard", "scopes": ["api:queue"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let bearer = minted["bearer"].as_str().expect("a bearer").to_owned();
    assert_eq!(
        minted["token"]["scopes"],
        serde_json::json!(["api:queue"]),
        "the stored token is wider than the request: {minted}"
    );

    // Queue control, which is what it asked for — and reading, which acting implies.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", &bearer).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Not credentials, which nothing confers.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/accounts", &bearer).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // And it appears in the inventory, or it could never be revoked.
    let (status, listing) =
        common::get_with_cookie(&harness.router, "/api/v1/api-tokens", &session).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(
        listing
            .as_array()
            .expect("a list")
            .iter()
            .any(|entry| entry["label"] == "Dashboard"),
        "a token minted for one area is invisible in the inventory: {listing}"
    );
}

/// A scope the model does not have must be refused, not dropped: a token quietly weaker than
/// the caller believes fails much later and somewhere else.
#[tokio::test]
async fn minting_refuses_a_scope_that_is_not_an_api_area() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let session = session(&harness).await;

    for scope in ["capture:*", "api:everything"] {
        let (status, body) = common::post_json_with_cookie(
            &harness.router,
            "/api/v1/api-tokens",
            &session,
            serde_json::json!({ "label": "Nope", "scopes": [scope] }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "`{scope}` was accepted: {body}"
        );
        assert_eq!(body["code"], "api.scope_unknown", "{body}");
    }
}

/// The numbers the token editor shows come from the table that enforces them.
#[tokio::test]
async fn the_capability_preview_describes_every_area() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let session = session(&harness).await;

    let (status, areas) =
        common::get_with_cookie(&harness.router, "/api/v1/api-tokens/scopes", &session).await;
    assert_eq!(status, StatusCode::OK, "{areas}");
    let areas = areas.as_array().expect("a list");
    // Six areas on the ladder and the metrics island beside it (RD-110-01).
    assert_eq!(areas.len(), 7, "{areas:?}");
    let metrics = areas
        .iter()
        .find(|area| area["scope"] == "api:metrics")
        .expect("the scrape area is offered");
    assert_eq!(metrics["operations"], 1, "{metrics}");
    assert_eq!(metrics["implies"], serde_json::json!([]), "{metrics}");
    assert!(
        areas
            .iter()
            .all(|area| area["operations"].as_u64().unwrap_or(0) > 0),
        "an area reaches nothing at all: {areas:?}"
    );
    let sensitive: Vec<_> = areas
        .iter()
        .filter(|area| area["sensitive"] == true)
        .map(|area| area["scope"].clone())
        .collect();
    assert_eq!(
        sensitive,
        vec![
            serde_json::json!("api:secrets"),
            serde_json::json!("api:admin")
        ],
        "the two areas nothing else confers must be the flagged ones"
    );
}

// `every_documented_route_enforces_its_scope` lived here and has moved to
// `tests/scope_matrix.rs`, which supersedes it: it walked every documented route with an
// `api:read` bearer and compared the result against a hand-copied duplicate of the route
// allowlist. The replacement derives its expectations from the policy table itself and covers
// every scope in both directions, so there is no second list to keep in step.

/// Finds a listed API token by the label the harness mints it under.
async fn token_id(harness: &common::Harness, label: &str) -> String {
    let (status, tokens) = get_with_bearer(&harness.router, "/api/v1/api-tokens", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    tokens
        .as_array()
        .expect("a list")
        .iter()
        .find(|token| token["label"] == label)
        .unwrap_or_else(|| panic!("no token labelled {label}: {tokens}"))["id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

/// The whole point of RD-106-11: a token's areas change and the *same* bearer reaches the new
/// surface on its very next request.
///
/// No restart, no reconnection, and nothing to invalidate — the scope lookup hashes the bearer
/// and reads the row on every request, so there is no cached copy to go stale. That is a
/// property of the design rather than an achievement of this change, which is exactly why it
/// is written down here: a cache added later would break this test rather than break a user.
#[tokio::test]
async fn widening_a_token_takes_effect_on_the_next_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let id = token_id(&harness, rd_core::API_READ_SCOPE).await;

    // Before: the reading token cannot see the settings document.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/settings", READ_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, token) = common::patch_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}"),
        API_BEARER,
        serde_json::json!({ "scopes": ["api:read", "api:config"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{token}");
    assert_eq!(token["id"], serde_json::json!(id), "{token}");
    assert_eq!(
        token["scopes"],
        serde_json::json!(["api:config", "api:read"]),
        "{token}"
    );
    // The response carries no bearer, because none was minted: the client keeps the value it
    // already has.
    assert!(token.get("bearer").is_none(), "{token}");

    let (status, body) = get_with_bearer(&harness.router, "/api/v1/settings", READ_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The half that matters more: taking an area away is refused at the *next* call, not after a
/// restart. A revocation that only takes hold on the next boot is not a revocation.
#[tokio::test]
async fn narrowing_a_token_is_refused_at_the_next_call_without_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let id = token_id(&harness, rd_core::API_SCOPE).await;

    let (status, body) = get_with_bearer(&harness.router, "/api/v1/categories", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, token) = common::patch_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}"),
        API_BEARER,
        serde_json::json!({ "scopes": ["api:read", "api:secrets"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{token}");

    // Same router, same bearer, same process — and configuration is gone already.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/categories", API_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    // Reading survived, which is what makes this a narrowing rather than a revocation.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The minting refusals are the same refusals here, or they were worth nothing.
///
/// `capture:*` is the one that matters: the capture surface and the API are isolated in both
/// directions, and re-scoping would be a second, quieter door into the same bridge.
#[tokio::test]
async fn re_scoping_refuses_what_minting_refuses() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let id = token_id(&harness, rd_core::API_READ_SCOPE).await;
    let uri = format!("/api/v1/api-tokens/{id}");

    for scopes in [
        serde_json::json!(["capture:*"]),
        serde_json::json!(["api:read", "capture:*"]),
        serde_json::json!(["api:everything"]),
        serde_json::json!(["api:Read"]),
    ] {
        let (status, body) = common::patch_with_bearer(
            &harness.router,
            &uri,
            API_BEARER,
            serde_json::json!({ "scopes": scopes }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{scopes}: {body}");
        assert_eq!(body["code"], "api.scope_unknown", "{scopes}: {body}");
    }

    // An empty list is refused rather than read as "nothing" or "everything"; removing all
    // access is spelled by revoking.
    let (status, body) = common::patch_with_bearer(
        &harness.router,
        &uri,
        API_BEARER,
        serde_json::json!({ "scopes": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "api.scopes_empty", "{body}");

    // And the token is untouched by any of it.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", READ_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A browser-capture token cannot be turned into an API token by naming its id.
///
/// `capture:*` being unmintable is only half of the isolation; the other half is that the
/// route refuses to touch a token the API token list does not show in the first place.
#[tokio::test]
async fn a_capture_token_cannot_be_rescoped_into_an_api_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let capture = harness
        .database
        .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
        .await
        .expect("capture tokens");
    let capture = capture.first().expect("the harness mints one");

    let (status, body) = common::patch_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{}", capture.id),
        API_BEARER,
        serde_json::json!({ "scopes": ["api:admin"] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "api.token_not_found", "{body}");

    // Unchanged where it counts: still a capture token, still no API access.
    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/downloads", CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// Re-scoping costs the same area as handing a credential out does.
#[tokio::test]
async fn re_scoping_needs_the_credentials_area() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let id = token_id(&harness, rd_core::API_READ_SCOPE).await;

    let (status, body) = common::patch_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}"),
        READ_BEARER,
        serde_json::json!({ "scopes": ["api:admin"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    assert_eq!(body["params"]["scope"], "api:secrets", "{body}");
}

/// Sending the same set twice changes nothing, and a restart finds what was saved.
///
/// Idempotence matters because the editor sends the complete set rather than a delta: a client
/// that retries a request it is unsure about must not end up somewhere else. And the change is
/// a row in the database rather than process state, so the second router built over the same
/// file — which is what a restart is here — has to agree with the first.
#[tokio::test]
async fn re_scoping_is_idempotent_and_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let id = token_id(&harness, rd_core::API_READ_SCOPE).await;
    let uri = format!("/api/v1/api-tokens/{id}");
    let body = serde_json::json!({ "scopes": ["api:read", "api:config"] });

    let (first_status, first) =
        common::patch_with_bearer(&harness.router, &uri, API_BEARER, body.clone()).await;
    let (second_status, second) =
        common::patch_with_bearer(&harness.router, &uri, API_BEARER, body).await;
    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert_eq!(second_status, StatusCode::OK, "{second}");
    assert_eq!(first, second, "the second call moved the token");

    // A second router over the same database file: the restart.
    let restarted = auth_harness(directory.path()).await;
    let (status, body) = get_with_bearer(&restarted.router, "/api/v1/settings", READ_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get_with_bearer(
        &restarted.router,
        "/api/v1/collector/candidates",
        READ_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// RD-130-12: the About page answers a status page's token, and nobody without a credential.
///
/// The health check stays public and says the version; the commit, the build time and the
/// dependency list are behind the sign-in, as the job's first acceptance criterion asks.
#[tokio::test]
async fn the_about_page_answers_a_read_token_and_nobody_signed_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    for uri in ["/api/v1/system/about", "/api/v1/system/about/licenses"] {
        let (status, body) = get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} signed out: {body}");
    }
    let (status, body) = get_json(&harness.router, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, about) =
        get_with_bearer(&harness.router, "/api/v1/system/about", READ_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{about}");
    assert_eq!(about["version"], body["version"], "{about}");
    // The test router is built without the binary's build script, so neither is known.
    assert!(about["commit"].is_null(), "{about}");
    assert!(about["built"].is_null(), "{about}");
    assert_eq!(about["license"], "GPL-3.0-or-later");
    let contracts = about["plugin_contracts"].as_array().expect("contracts");
    assert!(
        contracts.iter().all(|contract| contract
            .as_str()
            .is_some_and(|value| value.starts_with("rdownloader:plugin@"))),
        "{about}"
    );
    let links = about["links"].as_array().expect("links");
    assert_eq!(links.len(), 5, "{about}");
    let published: Vec<bool> = links.iter().map(|link| link["published"] == true).collect();
    // Every address is public since the first export with 1.3.0.
    assert_eq!(published, [true, true, true, true, true], "{about}");
    let tools: Vec<&str> = about["bundled_tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(tools.contains(&"Streamlink"), "{tools:?}");

    let (status, licenses) = get_with_bearer(
        &harness.router,
        "/api/v1/system/about/licenses",
        READ_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{licenses}");
    for ecosystem in ["rust", "npm"] {
        let entries = licenses[ecosystem].as_array().expect("a list");
        assert!(!entries.is_empty(), "{ecosystem} is empty");
        assert!(
            entries.iter().all(|entry| entry["license"]
                .as_str()
                .is_some_and(|license| !license.is_empty())),
            "{ecosystem} has an entry without a licence"
        );
    }
    assert!(
        licenses["rust"]
            .as_array()
            .expect("rust")
            .iter()
            .any(|entry| entry["name"] == "axum"),
        "the list carries the crates the service is built from"
    );
}
