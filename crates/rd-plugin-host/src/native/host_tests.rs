use std::sync::Arc;

use rd_core::{AccountId, FailureKind};
use rd_db::NewAccount;
use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostRequestValue};
use tokio::sync::RwLock;
use url::Url;

use super::super::expand::{
    allowed_header, content_type_is_json, cookie_scope, expand_request, expand_url,
    has_granted_secret_marker, http_failure, json_escape, method_allowed, provider_cookie_scope,
    validate_redirect,
};
use super::NativeHost;

/// `expand_request` with one value standing for the fixture's single reference.
fn expand_single(
    request: &mut HostHttpRequest,
    secret: Option<&str>,
    username: Option<&str>,
    client_id: Option<&str>,
    username_optional: bool,
) -> Result<bool, rd_core::Failure> {
    let secrets = crate::native::references::single_for_tests(request, secret);
    expand_request(request, &secrets, username, client_id, username_optional)
}

fn url(value: &str) -> Url {
    value.parse().expect("url")
}

async fn test_host(dir: &std::path::Path) -> NativeHost {
    let database = rd_db::Database::open(dir.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(dir.join("secrets"))
        .await
        .expect("secret store");
    NativeHost::new(
        database,
        ClientPool::default(),
        secrets,
        Arc::new(RwLock::new(NetworkDefaults::default())),
        None,
    )
}

async fn ddownload_account(
    host: &NativeHost,
    username: Option<&str>,
    api_key: Option<&str>,
) -> AccountId {
    let secret_ref = match api_key {
        Some(value) => Some(
            host.secrets
                .put_string(value.to_owned())
                .await
                .expect("put secret"),
        ),
        None => None,
    };
    host.database
        .create_account(NewAccount {
            provider: "ddownload".to_owned(),
            label: "Test".to_owned(),
            username: username.map(str::to_owned),
            credential_mode: None,
            secret_ref,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id
}

fn identity(account_id: Option<AccountId>) -> ClientIdentity {
    ClientIdentity {
        account_id,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// A `POST` with a JSON body, or with a form body when `json` is false: the two shapes the host
/// treats as templates (RD-120-66).
fn request(url: Url, body: Vec<u8>, json: bool) -> HostHttpRequest {
    crate::native::register_bundled_providers_for_tests();
    let headers = vec![HostRequestValue {
        name: "content-type".to_owned(),
        value_template: if json {
            "application/json"
        } else {
            "application/x-www-form-urlencoded"
        }
        .to_owned(),
    }];
    HostHttpRequest {
        method: "POST".to_owned(),
        url,
        query: Vec::new(),
        headers,
        body,
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    }
}

/// Resolves the request's secret and username the way `http_request` does, then runs the
/// same `expand_request` wiring on it. Returns the reported `carries_credential` flag.
async fn resolve_and_expand(
    host: &NativeHost,
    identity: &ClientIdentity,
    request: &mut HostHttpRequest,
) -> bool {
    let secrets = host
        .request_secrets(identity, request)
        .await
        .expect("secret resolved");
    let (username, username_optional) = host
        .request_username(identity, request)
        .await
        .expect("username resolved");
    let client_id = host
        .request_client_id(identity, request)
        .await
        .expect("client id resolved");
    expand_request(
        request,
        &secrets,
        username.as_deref(),
        client_id.as_deref(),
        username_optional,
    )
    .expect("expand")
}

// -- json_escape ------------------------------------------------------

#[test]
fn json_escape_escapes_quotes_backslashes_and_control_characters() {
    assert_eq!(json_escape(r#"a"b\c"#), r#"a\"b\\c"#);
    assert_eq!(json_escape("line1\nline2"), "line1\\nline2");
    assert_eq!(json_escape("\u{1}"), "\\u0001");
}

// -- content_type_is_json ------------------------------------------------

#[test]
fn content_type_is_json_matches_case_insensitively_with_parameters() {
    let json_headers = vec![HostRequestValue {
        name: "CONTENT-TYPE".to_owned(),
        value_template: "Application/JSON; charset=utf-8".to_owned(),
    }];
    assert!(content_type_is_json(&json_headers));

    let text_headers = vec![HostRequestValue {
        name: "content-type".to_owned(),
        value_template: "text/plain".to_owned(),
    }];
    assert!(!content_type_is_json(&text_headers));
    assert!(!content_type_is_json(&[]));
}

// -- allowed_header: browser-shaped headers for free flows ----------------

/// Free-download flows need `Referer`, `Origin` and `X-Requested-With` to pass a hoster's
/// form and countdown endpoints; nothing else may be added to the allowlist by accident.
#[test]
fn provenance_headers_are_allowed_but_arbitrary_ones_are_not() {
    for header in [
        "authorization",
        "content-type",
        "range",
        "user-agent",
        "accept",
        "referer",
        "Origin",
        "X-Requested-With",
    ] {
        assert!(allowed_header(header), "{header} must be allowed");
    }
    for header in [
        "cookie",
        "set-cookie",
        "x-forwarded-for",
        "host",
        // The host sets the length from the body it sends; a plugin that stated its own
        // could contradict it (RD-120-60, the Telegram notifier did).
        "content-length",
        "transfer-encoding",
    ] {
        assert!(!allowed_header(header), "{header} must stay blocked");
    }
}

/// `boxapi` and `hash` are allowed because Box's shared links and KrakenFiles' free download
/// need them, and only for what they carry there: a pasted link and a value off a public page.
/// No vault marker of any spelling may be expanded into either (RD-120-60).
#[test]
fn a_provider_header_carries_no_credential_marker() {
    assert!(allowed_header("BoxApi"));
    assert!(allowed_header("hash"));
    for name in ["boxapi", "BoxApi", "hash"] {
        for template in [
            "shared_link=https://app.box.com/s/abc&shared_link_password={{secret:box_token}}",
            "{{secret}}",
            "{{basic:box_token}}",
            "{{username}}",
        ] {
            let mut request = HostHttpRequest {
                method: "GET".to_owned(),
                url: url::Url::parse("https://api.box.com/2.0/shared_items").expect("url"),
                query: Vec::new(),
                headers: vec![HostRequestValue {
                    name: name.to_owned(),
                    value_template: template.to_owned(),
                }],
                body: Vec::new(),
                granted_secret: Some("granted".to_owned()),
                authority: rd_plugin_api::RequestAuthority::Provider,
                write_methods: false,
            };
            let error = expand_single(
                &mut request,
                Some("secret-value"),
                Some("user"),
                None,
                false,
            )
            .expect_err("a credential in a provider header must be refused");
            assert_eq!(
                error.code.as_deref(),
                Some("plugin.secret_target_not_allowed"),
                "{name} / {template}"
            );
        }
    }
}

/// The provenance headers travel to third-party hosts, so a plugin must not be able to
/// smuggle the account secret or username into one.
#[test]
fn credential_markers_are_rejected_in_provenance_headers() {
    for name in ["Referer", "origin", "x-requested-with"] {
        for template in ["{{secret:ddownload_api_key}}", "{{username}}"] {
            let mut request = HostHttpRequest {
                method: "GET".to_owned(),
                url: url::Url::parse("https://ddownload.com/file").expect("url"),
                query: Vec::new(),
                headers: vec![HostRequestValue {
                    name: name.to_owned(),
                    value_template: template.to_owned(),
                }],
                body: Vec::new(),
                granted_secret: None,
                authority: rd_plugin_api::RequestAuthority::Provider,
                write_methods: false,
            };

            let error = expand_single(
                &mut request,
                Some("secret-value"),
                Some("user"),
                None,
                false,
            )
            .expect_err("credential in a provenance header must be refused");

            assert_eq!(
                error.code.as_deref(),
                Some("plugin.secret_target_not_allowed"),
                "{name} / {template}"
            );
        }
    }
}

/// The allowlisted credential carriers keep working unchanged.
#[test]
fn credential_markers_still_expand_in_authorization() {
    let mut request = HostHttpRequest {
        method: "GET".to_owned(),
        url: url::Url::parse("https://ddownload.com/file").expect("url"),
        query: Vec::new(),
        headers: vec![HostRequestValue {
            name: "authorization".to_owned(),
            value_template: "Bearer {{secret:ddownload_api_key}}".to_owned(),
        }],
        body: Vec::new(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    };

    expand_single(&mut request, Some("secret-value"), None, None, false).expect("expand");

    assert_eq!(request.headers[0].value_template, "Bearer secret-value");
}

// -- expand_request: body wiring -----------------------------------------
//
// These call `expand_request` directly (the function `http_request` itself calls) rather
// than the lower-level `expand_template`/`expand_username`, so they exercise the real
// `content-type` sniffing and `request.body` rewrite, not a hand-picked `escape` literal.

#[test]
fn expand_request_json_escapes_a_secret_in_the_body_from_a_mixed_case_content_type() {
    let mut request = HostHttpRequest {
        method: "POST".to_owned(),
        url: url("https://api-v2.ddownload.com/api/account/info"),
        query: Vec::new(),
        headers: vec![HostRequestValue {
            name: "Content-Type".to_owned(),
            value_template: "Application/JSON; charset=utf-8".to_owned(),
        }],
        body: br#"{"api_key":"{{secret:ddownload_api_key}}"}"#.to_vec(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    };

    let carries_credential = expand_single(
        &mut request,
        Some(r#"key"with\backslash"#),
        None,
        None,
        false,
    )
    .expect("expand");

    assert!(carries_credential);
    assert_eq!(
        request.body,
        br#"{"api_key":"key\"with\\backslash"}"#.to_vec()
    );
}

/// A body with no declared type is content, not a template (RD-120-66): it used to be
/// substituted verbatim.
#[test]
fn expand_request_leaves_an_undeclared_body_unexpanded() {
    let mut request = HostHttpRequest {
        method: "POST".to_owned(),
        url: url("https://api-v2.ddownload.com/api/account/info"),
        query: Vec::new(),
        headers: Vec::new(),
        body: b"api_key={{secret:ddownload_api_key}}".to_vec(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    };

    expand_single(&mut request, Some("plain-key"), None, None, false).expect("expand");

    assert_eq!(
        request.body,
        b"api_key={{secret:ddownload_api_key}}".to_vec()
    );
}

#[test]
fn expand_request_with_neither_secret_nor_username_does_not_carry_credentials() {
    let mut request = HostHttpRequest {
        method: "GET".to_owned(),
        url: url("https://api-v2.ddownload.com/api/account/info"),
        query: Vec::new(),
        headers: Vec::new(),
        body: b"plain body".to_vec(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    };

    let carries_credential = expand_single(&mut request, None, None, None, false).expect("expand");

    assert!(!carries_credential);
    assert_eq!(request.body, b"plain body".to_vec());
}

// -- body secret expansion, through the real request_secrets + expand_request pipeline ------

#[tokio::test]
async fn secret_marker_in_json_body_is_expanded_with_json_escaping() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, None, Some(r#"key"with\backslash"#)).await;
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        br#"{"api_key":"{{secret:ddownload_api_key}}"}"#.to_vec(),
        true,
    );

    let carries_credential =
        resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert!(carries_credential);
    assert_eq!(
        request.body,
        br#"{"api_key":"key\"with\\backslash"}"#.to_vec()
    );
}

#[tokio::test]
async fn secret_marker_in_a_form_body_is_substituted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, None, Some("plain-key")).await;
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        b"api_key={{secret:ddownload_api_key}}".to_vec(),
        false,
    );

    resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert_eq!(request.body, b"api_key=plain-key".to_vec());
}

#[tokio::test]
async fn body_secret_marker_with_disallowed_domain_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, None, Some("plain-key")).await;
    // ddownload.com (not api-v2.ddownload.com) is not in the secret's allowed domains.
    let request = request(
        url("https://ddownload.com/"),
        b"{{secret:ddownload_api_key}}".to_vec(),
        false,
    );

    let error = host
        .request_secrets(&identity(Some(account_id)), &request)
        .await
        .expect_err("target not allowed");

    assert_eq!(
        error.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}

#[tokio::test]
async fn binary_body_without_markers_is_left_untouched() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, None, Some("plain-key")).await;
    let binary_body = vec![0xff, 0x00, 0xfe, 0x01, 0x80];
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        binary_body.clone(),
        false,
    );

    let carries_credential =
        resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert!(!carries_credential);
    assert_eq!(request.body, binary_body);
    assert!(
        std::str::from_utf8(&binary_body).is_err(),
        "fixture is not valid UTF-8"
    );
}

// -- {{username}}, through the real request_username + expand_request pipeline -------------

#[tokio::test]
async fn username_is_expanded_in_query_and_body() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, Some("alice"), Some("plain-key")).await;
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        br#"{"user":"{{username}}"}"#.to_vec(),
        true,
    );
    request.query.push(HostRequestValue {
        name: "user".to_owned(),
        value_template: "{{username}}".to_owned(),
    });

    let carries_credential =
        resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert!(carries_credential);
    assert_eq!(request.query[0].value_template, "alice");
    assert_eq!(request.body, br#"{"user":"alice"}"#.to_vec());
}

#[tokio::test]
async fn missing_username_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, None, Some("plain-key")).await;
    let request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        b"{{username}}".to_vec(),
        false,
    );

    let error = host
        .request_username(&identity(Some(account_id)), &request)
        .await
        .expect_err("username missing");

    assert_eq!(error.code.as_deref(), Some("plugin.username_missing"));
    assert_eq!(error.category, FailureKind::AuthRequired);
}

#[tokio::test]
async fn username_marker_without_account_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        b"{{username}}".to_vec(),
        false,
    );

    let error = host
        .request_username(&identity(None), &request)
        .await
        .expect_err("account missing");

    assert_eq!(error.code.as_deref(), Some("plugin.account_missing"));
}

#[tokio::test]
async fn username_in_json_body_is_escaped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, Some(r#"a"b"#), Some("plain-key")).await;
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        br#"{"user":"{{username}}"}"#.to_vec(),
        true,
    );

    resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert_eq!(request.body, br#"{"user":"a\"b"}"#.to_vec());
}

// -- cookie / redirect ---------------------------------------------------

#[test]
fn cookie_scope_is_the_provider_domain_not_the_request_host() {
    crate::native::register_bundled_providers_for_tests();
    let api = url("https://api-v2.ddownload.com/api/account/info");
    assert_eq!(
        cookie_scope(Some("ddownload"), &api).as_str(),
        "https://ddownload.com/"
    );
    assert_eq!(cookie_scope(None, &api), api);
    assert!(provider_cookie_scope("unknown").is_none());
}

#[test]
fn plain_requests_may_follow_https_redirects_to_the_cdn() {
    let source = url("https://ddownload.com/abc123xyz");
    let cdn = url("https://eu-hydra5.zeuscdn.org:183/d/token/file.rar");
    assert!(validate_redirect(&source, &cdn, false).is_ok());
    assert!(validate_redirect(&source, &url("http://eu-hydra5.zeuscdn.org/d/x"), false).is_err());
    assert!(validate_redirect(&source, &url("https://ddownload.com/?op=payments"), true).is_ok());
}

#[test]
fn requests_carrying_a_secret_must_stay_on_manifest_domains() {
    crate::native::register_bundled_providers_for_tests();
    let source = url("https://api-v2.ddownload.com/api/file/info");
    assert!(validate_redirect(&source, &url("https://example.com/"), true).is_err());
    assert!(validate_redirect(&source, &url("https://ddownload.com/"), true).is_ok());
}

/// End-to-end: a request whose only marker is `{{username}}` (no secret) still trips the
/// redirect hardening, because `expand_request`'s reported `carries_credential` — not a
/// hand-picked literal — feeds `validate_redirect`. If `carries_credential` regressed to
/// `secret.is_some()` alone, this would fail.
#[tokio::test]
async fn username_only_request_triggers_redirect_hardening_end_to_end() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account_id = ddownload_account(&host, Some("alice"), None).await;
    let mut request = request(
        url("https://api-v2.ddownload.com/api/account/info"),
        b"{{username}}".to_vec(),
        false,
    );

    let secret = host
        .request_secrets(&identity(Some(account_id)), &request)
        .await
        .expect("no secret marker present");
    assert!(secret.is_empty(), "fixture must not carry a secret marker");
    let carries_credential =
        resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert!(
        carries_credential,
        "a username-only request must be reported as carrying credential material"
    );
    assert_eq!(request.body, b"alice");

    let source = url("https://api-v2.ddownload.com/api/file/info");
    assert!(validate_redirect(&source, &url("https://example.com/"), carries_credential).is_err());
    assert!(validate_redirect(&source, &url("https://ddownload.com/"), carries_credential).is_ok());
}

/// A transport error (DNS failure, reset, refused connection, ...) carries the fully
/// expanded request URL, which for plugins like rapidgator/nitroflare/linksnappy embeds the
/// account password or premium key in the query string. `http_failure` must strip it from
/// both the persisted message and the `error` param before the `Failure` is stored or shown
/// in the UI.
#[tokio::test]
async fn http_failure_strips_credentials_from_the_url() {
    let client = reqwest::Client::new();
    // Port 0 on loopback is never listening, so this fails fast and locally (connection
    // refused) without depending on external network or DNS.
    let result = client
        .get("http://127.0.0.1:0/api/login?login=me%40example.com&password=hunter2")
        .send()
        .await;
    let error = result.expect_err("connecting to a closed local port must fail");
    assert!(
        error.url().is_some(),
        "sanity check: the source error must actually carry a url"
    );

    let failure = http_failure(error);

    assert!(
        !failure.message.contains("hunter2"),
        "message leaked the password: {}",
        failure.message
    );
    assert!(
        !failure.message.contains('?'),
        "message leaked the query string: {}",
        failure.message
    );
    let error_param = failure.params.get("error").expect("error param present");
    assert!(
        !error_param.contains("hunter2"),
        "error param leaked the password: {error_param}"
    );
    assert!(
        !error_param.contains('?'),
        "error param leaked the query string: {error_param}"
    );
}

// -- the reference-less `{{secret}}` marker -------------------------------
//
// The plugin types with no provider account behind them reach their credential this way. What
// keeps it safe is that the host chooses the reference, so these tests are about what happens
// when a plugin writes the marker without having been granted anything.

#[test]
fn the_reference_less_marker_is_found_anywhere_in_a_request() {
    let mut plain = request(url("https://discord.com/api/webhooks/x"), Vec::new(), false);
    assert!(!has_granted_secret_marker(&plain));

    plain.headers.push(HostRequestValue {
        name: "authorization".to_owned(),
        value_template: "Bearer {{secret}}".to_owned(),
    });
    assert!(has_granted_secret_marker(&plain));

    // Also in the address itself, which a webhook whose token is part of its path needs.
    let in_url = request(
        url("https://discord.com/api/webhooks/{{secret}}"),
        Vec::new(),
        false,
    );
    assert!(has_granted_secret_marker(&in_url));
}

#[test]
fn an_ungranted_marker_expands_to_nothing_rather_than_an_empty_value() {
    // Sending the request with the marker still in it, or with an empty value where a token
    // should be, would both look like a configuration mistake somewhere else entirely.
    let mut ungranted = request(url("https://ntfy.sh/topic"), Vec::new(), false);
    ungranted.headers.push(HostRequestValue {
        name: "authorization".to_owned(),
        value_template: "Bearer {{secret}}".to_owned(),
    });
    let error =
        expand_single(&mut ungranted, None, None, None, false).expect_err("no secret was granted");
    assert_eq!(error.category, FailureKind::AuthRequired);
}

#[test]
fn a_secret_in_an_address_may_lengthen_the_path_but_not_move_the_host() {
    let base = url("https://discord.com/api/webhooks/{{secret}}");
    let expanded = expand_url(&base, Some("123/abc-token")).expect("expand");
    assert_eq!(
        expanded.as_str(),
        "https://discord.com/api/webhooks/123/abc-token"
    );

    // A value substituted into a path stays in the path — the authority was already fixed
    // when the domain gate ran. The check in `expand_url` is what makes that a verified
    // property of every expansion rather than an assumption about the URL parser.
    let hostile = expand_url(&base, Some("123/token@evil.example")).expect("still on discord");
    assert_eq!(hostile.host_str(), Some("discord.com"));
    assert!(expand_url(&base, None).is_err(), "nothing was granted");
}

// -- the renewal material of an OAuth account (RD-106-03) -----------------
//
// A renewal is the one request whose credential is neither the account's own secret nor a
// granted one: the host minted the reference in `store_oauth_token` and handed it back to the
// plugin as `credential-ref`, and the plugin writes it as `{{secret:<reference>}}`. Before
// this branch existed the marker was refused as a target the provider does not declare, so
// every OAuth renewal failed and nobody could stay signed in.

/// An account of the Real-Debrid provider with a stored renewal reference, and the reference.
async fn signed_in_account(host: &NativeHost) -> (AccountId, String) {
    crate::native::register_bundled_providers_for_tests();
    let account_id = host
        .database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Test".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some(
                host.secrets
                    .put_string("the-access-token".to_owned())
                    .await
                    .expect("put access token"),
            ),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id;
    let refresh_ref = host
        .secrets
        .put_string("the-refresh-token".to_owned())
        .await
        .expect("put refresh token");
    host.database
        .upsert_auth_flow(rd_db::UpsertAuthFlow {
            account_id,
            plugin_id: "019d0000-0000-7000-8000-000000000113".to_owned(),
            state: rd_core::AuthFlowState::Authorized,
            verification_url: None,
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: None,
            refresh_ref: Some(refresh_ref.clone()),
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: None,
        })
        .await
        .expect("store the flow");
    (account_id, refresh_ref)
}

fn renewal_request(reference: &str) -> HostHttpRequest {
    let mut request = request(
        url("https://api.real-debrid.com/oauth/v2/token"),
        Vec::new(),
        false,
    );
    request.query.push(HostRequestValue {
        name: "code".to_owned(),
        value_template: format!("{{{{secret:{reference}}}}}"),
    });
    request
}

/// The renewal material reaches the request, and it is the refresh token rather than the
/// access token the account also holds. Getting that wrong would send the wrong credential to
/// the provider on every renewal and look like a revoked sign-in.
#[tokio::test]
async fn an_oauth_renewal_expands_the_material_the_host_handed_the_plugin() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let (account_id, refresh_ref) = signed_in_account(&host).await;

    let mut request = renewal_request(&refresh_ref);
    let carries_credential =
        resolve_and_expand(&host, &identity(Some(account_id)), &mut request).await;

    assert!(carries_credential);
    assert_eq!(request.query[0].value_template, "the-refresh-token");
}

/// And nothing else. A plugin that named a vault reference it was never handed finds nothing,
/// even one belonging to another account of the same provider.
#[tokio::test]
async fn a_vault_reference_the_plugin_was_not_handed_is_refused() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let (account_id, _) = signed_in_account(&host).await;
    let (_, other_reference) = signed_in_account(&host).await;

    let error = host
        .request_secrets(
            &identity(Some(account_id)),
            &renewal_request(&other_reference),
        )
        .await
        .expect_err("another account's renewal material must not be reachable");
    assert_eq!(
        error.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}

/// The address gate still applies. A renewal reference is credential material like any other,
/// so it may only go where the provider's own credential may go.
#[tokio::test]
async fn the_renewal_material_may_not_be_sent_outside_the_providers_hosts() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let (account_id, refresh_ref) = signed_in_account(&host).await;

    let mut elsewhere = renewal_request(&refresh_ref);
    elsewhere.url = url("https://collector.example/collect");

    let error = host
        .request_secrets(&identity(Some(account_id)), &elsewhere)
        .await
        .expect_err("a renewal must not reach another host");
    assert_eq!(
        error.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}

// -- an account that registered its own OAuth application (RD-106-03) ------
//
// Two credentials have to exist at once: the client secret the person typed, and the access
// token the sign-in obtained. They used to be the same stored value, so the first successful
// sign-in destroyed the registration that every later renewal needs.

/// A Real-Debrid account carrying the application the person registered, and a started flow.
async fn account_with_a_registered_application(host: &NativeHost) -> AccountId {
    // The provider rows come from the bundled manifests, and `store_oauth_token` reads this
    // account's row to learn that its token has a slot of its own. Registering here rather
    // than relying on another test having done it keeps this file order-independent.
    crate::native::register_bundled_providers_for_tests();
    let secret_ref = host
        .secrets
        .put_string("the-client-secret".to_owned())
        .await
        .expect("put the client secret");
    let account_id = host
        .database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Test".to_owned(),
            username: Some("the-client-id".to_owned()),
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id;
    host.database
        .upsert_auth_flow(rd_db::UpsertAuthFlow {
            account_id,
            plugin_id: "019d0000-0000-7000-8000-000000000113".to_owned(),
            state: rd_core::AuthFlowState::WaitingForUser,
            verification_url: Some("https://real-debrid.com/device".to_owned()),
            user_code: Some("WXYZ1234".to_owned()),
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: None,
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: Some("a-device-code".to_owned()),
        })
        .await
        .expect("start the flow");
    account_id
}

/// What one marker expands to for this account, through the real resolution pipeline.
async fn expanded(host: &NativeHost, account_id: AccountId, template: &str) -> String {
    let mut request = request(
        url("https://api.real-debrid.com/oauth/v2/token"),
        Vec::new(),
        false,
    );
    request.query.push(HostRequestValue {
        name: "value".to_owned(),
        value_template: template.to_owned(),
    });
    resolve_and_expand(host, &identity(Some(account_id)), &mut request).await;
    request.query[0].value_template.clone()
}

/// The sign-in stores its token beside the flow and leaves the registration alone, so both are
/// reachable afterwards — the point of the whole two-slot arrangement.
#[tokio::test]
async fn an_oauth_sign_in_keeps_the_registration_and_the_token_apart() {
    use rd_plugin_api::ResolverHost as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = account_with_a_registered_application(&host).await;

    host.store_oauth_token(
        account_id,
        "the-access-token",
        Some("the-refresh-token"),
        Some(3600),
    )
    .await
    .expect("store what the exchange produced");

    // The registration survived. Before RD-106-03 this line read "the-access-token", and the
    // next renewal had nothing to sign itself with.
    assert_eq!(
        expanded(&host, account_id, "{{secret:realdebrid_client_secret}}").await,
        "the-client-secret"
    );
    // And the token is reachable under its own slot, which is what the resolver asks for.
    assert_eq!(
        expanded(&host, account_id, "{{secret:realdebrid_access_token}}").await,
        "the-access-token"
    );
    // The client id is an identifier rather than a secret, so it travels as the username —
    // held to the same hosts, because it is credential material all the same.
    assert_eq!(
        expanded(&host, account_id, "{{username}}").await,
        "the-client-id"
    );
}

/// A slot the flow fills answers about the flow, not about the account. Answering from the
/// account's credential would report an account as signed in the moment it registered an
/// application — and a resolver believing it would send the client secret as a Bearer token.
#[tokio::test]
async fn a_flow_filled_slot_is_unavailable_until_the_flow_has_filled_it() {
    use rd_plugin_api::ResolverHost as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = account_with_a_registered_application(&host).await;

    assert!(
        host.secret_available(account_id, "realdebrid_client_secret")
            .await,
        "the registration is there from the start"
    );
    assert!(
        !host
            .secret_available(account_id, "realdebrid_access_token")
            .await,
        "nothing has signed in yet"
    );

    host.store_oauth_token(account_id, "the-access-token", None, Some(3600))
        .await
        .expect("store what the exchange produced");

    assert!(
        host.secret_available(account_id, "realdebrid_access_token")
            .await
    );
}

/// A second sign-in replaces the token and still leaves the registration alone. The old token
/// is dropped only after the new one is referenced, the order the rest of this file keeps.
#[tokio::test]
async fn signing_in_again_replaces_the_token_and_nothing_else() {
    use rd_plugin_api::ResolverHost as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let host = test_host(directory.path()).await;
    let account_id = account_with_a_registered_application(&host).await;

    for token in ["first-token", "second-token"] {
        host.store_oauth_token(account_id, token, Some("the-refresh-token"), Some(3600))
            .await
            .expect("store what the exchange produced");
    }

    assert_eq!(
        expanded(&host, account_id, "{{secret:realdebrid_access_token}}").await,
        "second-token"
    );
    assert_eq!(
        expanded(&host, account_id, "{{secret:realdebrid_client_secret}}").await,
        "the-client-secret"
    );
}

/// RD-107-05, host gap 1: `PROPFIND` reads a collection, so a folder crawler may send it
/// without being handed the methods that change what is at the far end.
///
/// Both halves matter. Before this, a crawler that wanted to list a WebDAV share had only one
/// way in — the write gate — and taking it would have given it `PUT` and `DELETE` as well.
#[test]
fn propfind_is_a_read_and_the_writing_methods_are_not() {
    // A crawler: no write methods at all.
    assert!(method_allowed("PROPFIND", false), "a listing is a read");
    assert!(method_allowed("GET", false));
    assert!(method_allowed("POST", false));
    assert!(method_allowed("HEAD", false));
    for method in ["PUT", "DELETE", "MKCOL"] {
        assert!(
            !method_allowed(method, false),
            "{method} changes the far end and must stay behind the write gate"
        );
    }
    // A storage destination: the write methods, and nothing new beyond them.
    for method in ["PUT", "DELETE", "MKCOL", "PROPFIND", "GET"] {
        assert!(method_allowed(method, true), "{method} is a storage method");
    }
    // Nothing widened the list itself.
    for method in ["PATCH", "COPY", "MOVE", "LOCK", "TRACE"] {
        assert!(!method_allowed(method, false));
        assert!(!method_allowed(method, true));
    }
}

/// A `PROPFIND` without `Depth` is served at the server's default, which for a collection is
/// either everything or nothing — so the header a listing needs has to be sendable.
#[test]
fn the_depth_header_a_listing_needs_is_allowed() {
    assert!(allowed_header("Depth"));
    assert!(allowed_header("depth"));
    // The header Nextcloud demands for everything but `GET` on its public DAV endpoint.
    assert!(allowed_header("X-Requested-With"));
    // And nothing else came along with them.
    assert!(!allowed_header("Destination"));
    assert!(!allowed_header("Overwrite"));
}
