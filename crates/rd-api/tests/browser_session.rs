//! RD-120-45: a browser's session at a provider, handed over to one account.
//!
//! A person asks in the web interface, the extension answers with its capture token, and the
//! cookies end up where cookies typed into the account form end up: one vault entry behind
//! `accounts.cookie_ref`. The extension can neither start a request nor pick its domain.

mod common;

use axum::http::StatusCode;
use common::{
    CAPTURE_BEARER, delete_json, get_json, get_with_bearer, post_json, post_with_bearer,
    test_harness,
};
use rd_provider_registry::{
    CredentialKind, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec, SecretFilledBy,
    SecretSlot, TransferAuth, replace_dynamic,
};

/// The cookie value the tests hand over; it may appear in no response.
const SESSION: &str = "xfss-session-value-never-echoed";

/// A hoster whose plugin declares `cookie_scope`, the way DDownload's manifest does.
fn install(slug: &str, cookie_scope: Option<&str>) {
    replace_dynamic(vec![DynamicProvider {
        plugin_id: format!("plugin-{slug}"),
        spec: ProviderSpec {
            slug: slug.to_owned(),
            display_name: format!("Fixture {slug}"),
            kind: ProviderKind::Hoster,
            credentials: CredentialKind::ApiKey,
            username_required: false,
            transfer_auth: TransferAuth::None,
            secrets: vec![SecretSlot {
                reference: format!("{slug}_api_key"),
                domains: vec![format!("api.{slug}.test")],
                mode: None,
                filled_by: SecretFilledBy::Person,
            }],
            request_domains: vec![format!("{slug}.test")],
            cookie_scope: cookie_scope.map(str::to_owned),
            match_hosts: vec![format!("{slug}.test")],
            host_aliases: Vec::new(),
            source: ProviderSource::Plugin,
            plugin_id: Some(format!("plugin-{slug}")),
            plugin_version: Some("1.0.0".to_owned()),
        },
    }]);
}

async fn create_account(router: &axum::Router, provider: &str) -> String {
    let (status, account) = post_json(
        router,
        "/api/v1/accounts",
        serde_json::json!({
            "provider": provider,
            "label": "Main",
            "username": null,
            "credential_mode": null,
            "secret": "api-key",
            "cookies": null,
            "proxy_profile_id": null,
            "enabled": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{account}");
    assert_eq!(account["has_cookies"], false);
    account["id"].as_str().expect("id").to_owned()
}

/// One test, because the registry is process-wide and the steps depend on each other.
#[tokio::test]
async fn a_requested_session_is_stored_on_the_account_and_nothing_else_is_accepted() {
    let directory = tempfile::tempdir().expect("directory");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    install("fixturehost", Some("https://fixturehost.test/"));
    let account = create_account(router, "fixturehost").await;

    // Nothing is offered to the extension before a person asks.
    let (status, waiting) =
        get_with_bearer(router, "/api/v1/capture/browser-sessions", CAPTURE_BEARER).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(waiting, serde_json::json!([]));

    let (status, begun) = post_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{begun}");
    assert_eq!(begun["state"], "waiting");
    assert_eq!(begun["host"], "fixturehost.test");
    let id = begun["id"].as_str().expect("id").to_owned();

    // The extension learns the scope from the service, together with whose account it is for.
    let (_, waiting) =
        get_with_bearer(router, "/api/v1/capture/browser-sessions", CAPTURE_BEARER).await;
    assert_eq!(waiting[0]["id"], id.as_str());
    assert_eq!(waiting[0]["scope"], "https://fixturehost.test/");
    assert_eq!(waiting[0]["host"], "fixturehost.test");
    assert_eq!(waiting[0]["account_label"], "Main");
    assert_eq!(waiting[0]["provider_name"], "Fixture fixturehost");

    // One row of another site refuses the whole set, and nothing is stored.
    let (status, refused) = post_with_bearer(
        router,
        &format!("/api/v1/capture/browser-sessions/{id}"),
        CAPTURE_BEARER,
        serde_json::json!({
            "cookies": format!(
                ".fixturehost.test\tTRUE\t/\tTRUE\t0\txfss\t{SESSION}\n.other.test\tTRUE\t/\tTRUE\t0\tsid\tx"
            ),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "browser_session.cookie_outside_scope");
    let (_, stored) = harness
        .database
        .account_secret_refs(account.parse().expect("account id"))
        .await
        .expect("refs")
        .expect("account");
    assert!(
        stored.is_none(),
        "a refused set leaves the account as it was"
    );

    // The same request still takes a set of its own scope.
    let cookies = format!(
        ".fixturehost.test\tTRUE\t/\tTRUE\t2000000000\txfss\t{SESSION}\n\
         #HttpOnly_fixturehost.test\tFALSE\t/\tTRUE\t0\tlogin\tme"
    );
    let (status, delivered) = post_with_bearer(
        router,
        &format!("/api/v1/capture/browser-sessions/{id}"),
        CAPTURE_BEARER,
        serde_json::json!({ "cookies": cookies }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delivered}");
    assert_eq!(delivered["code"], "browser_session.delivered");
    assert_eq!(delivered["params"]["count"], "2");
    assert!(
        !delivered.to_string().contains(SESSION),
        "the cookies were echoed"
    );

    // Stored exactly as a cookie import is: a vault entry behind the account's cookie_ref.
    let (_, cookie_ref) = harness
        .database
        .account_secret_refs(account.parse().expect("account id"))
        .await
        .expect("refs")
        .expect("account");
    let cookie_ref = cookie_ref.expect("cookie_ref set");
    let stored = harness.secrets.get(&cookie_ref).await.expect("vault entry");
    assert_eq!(secrecy::ExposeSecret::expose_secret(&stored), cookies);
    let (_, accounts) = get_json(router, "/api/v1/accounts").await;
    assert_eq!(accounts[0]["has_cookies"], true);
    assert!(!accounts.to_string().contains(SESSION));

    let (_, status_now) = get_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
    )
    .await;
    assert_eq!(status_now["state"], "delivered");

    // Answered once: neither a second delivery nor a late decline is taken.
    let (status, again) = post_with_bearer(
        router,
        &format!("/api/v1/capture/browser-sessions/{id}"),
        CAPTURE_BEARER,
        serde_json::json!({ "cookies": cookies }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{again}");
    assert_eq!(again["code"], "browser_session.not_waiting");

    // A second request can be declined in the extension, and withdrawn from the interface.
    let (_, second) = post_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
        serde_json::Value::Null,
    )
    .await;
    let second = second["id"].as_str().expect("id").to_owned();
    let (status, declined) = post_with_bearer(
        router,
        &format!("/api/v1/capture/browser-sessions/{second}/decline"),
        CAPTURE_BEARER,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{declined}");
    let (_, status_now) = get_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
    )
    .await;
    assert_eq!(status_now["state"], "declined");
    let (status, _) = delete_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, none) = get_json(
        router,
        &format!("/api/v1/accounts/{account}/browser-session"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(none["code"], "browser_session.none");

    // A provider without a cookie_scope takes no session from a browser at all.
    install("scopeless", None);
    let other = create_account(router, "scopeless").await;
    let (status, refused) = post_json(
        router,
        &format!("/api/v1/accounts/{other}/browser-session"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "browser_session.no_cookie_scope");
    replace_dynamic(Vec::new());
}
