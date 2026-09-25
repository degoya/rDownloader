//! Two vault references in one request, each filled with its own value (RD-120-39).
//!
//! The host tests run through the real `NativeHost`: a real database, a real vault and the
//! bundled Real-Debrid provider rows, and the same `request_secrets` + `expand_request` wiring
//! `http_request` uses before it sends. The two values are canaries, so a test can say not only
//! that each field holds the right value but that neither value is anywhere it was not named.

use std::sync::Arc;

use rd_core::AccountId;
use rd_db::NewAccount;
use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostRequestValue};
use secrecy::SecretString;
use tokio::sync::RwLock;

use super::super::expand::{Escape, expand_request, expand_template};
use super::super::host::NativeHost;
use super::*;

const CLIENT_SECRET: &str = "CANARY-A-client-secret-5d1e";
const REFRESH_TOKEN: &str = "CANARY-B-refresh-token-8c47";
const CLIENT_ID: &str = "the-client-id";
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";
/// `plugins/realdebrid-auth`'s grant type, copied so the replay is the request it sends.
const GRANT_TYPE: &str = "http://oauth.net/grant_type/device/1.0";

fn value(name: &str, template: &str) -> HostRequestValue {
    HostRequestValue {
        name: name.to_owned(),
        value_template: template.to_owned(),
    }
}

fn secret(reference: &str) -> String {
    format!("{{{{secret:{reference}}}}}")
}

fn request(
    query: Vec<HostRequestValue>,
    headers: Vec<HostRequestValue>,
    body: &str,
) -> HostHttpRequest {
    HostHttpRequest {
        method: "POST".to_owned(),
        url: TOKEN_ENDPOINT.parse().expect("url"),
        query,
        headers,
        body: body.as_bytes().to_vec(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    }
}

async fn test_host(dir: &std::path::Path) -> NativeHost {
    crate::native::register_bundled_providers_for_tests();
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

/// A Real-Debrid account whose person registered their own application and has signed in:
/// the account's secret is the client secret, the flow row holds the refresh material. That
/// is the state in which `realdebrid-auth` renews, and the request it renews with names both.
async fn registered_and_signed_in(host: &NativeHost, refresh_token: &str) -> (AccountId, String) {
    let client_secret_ref = host
        .secrets
        .put_string(CLIENT_SECRET.to_owned())
        .await
        .expect("put the client secret");
    let account_id = host
        .database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Test".to_owned(),
            username: Some(CLIENT_ID.to_owned()),
            credential_mode: None,
            secret_ref: Some(client_secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id;
    let refresh_ref = host
        .secrets
        .put_string(refresh_token.to_owned())
        .await
        .expect("put the refresh token");
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

fn identity(account_id: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account_id),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// What `http_request` does before it sends: resolve every credential, then expand.
async fn expand(
    host: &NativeHost,
    identity: &ClientIdentity,
    request: &mut HostHttpRequest,
) -> Result<(), rd_core::Failure> {
    let secrets = host.request_secrets(identity, request).await?;
    let (username, optional) = host.request_username(identity, request).await?;
    expand_request(request, &secrets, username.as_deref(), None, optional).map(|_| ())
}

fn field<'a>(values: &'a [HostRequestValue], name: &str) -> &'a str {
    values
        .iter()
        .find(|value| value.name == name)
        .map(|value| value.value_template.as_str())
        .expect("field present")
}

/// Every place a value can travel in: each query and header value, and the body.
fn wire(request: &HostHttpRequest) -> Vec<String> {
    request
        .query
        .iter()
        .chain(&request.headers)
        .map(|value| value.value_template.clone())
        .chain(std::iter::once(
            String::from_utf8(request.body.clone()).expect("utf-8 body"),
        ))
        .collect()
}

fn occurrences(request: &HostHttpRequest, needle: &str) -> usize {
    wire(request)
        .iter()
        .map(|text| text.matches(needle).count())
        .sum()
}

// -- through the real host ---------------------------------------------------

/// `realdebrid-auth`'s `exchange()` as it renews: the application's client secret and the
/// stored refresh material, two references in one query. Before RD-120-39 the host loaded the
/// first one and wrote it into both, so the renewal sent the client secret as the `code`.
#[tokio::test]
async fn the_real_debrid_renewal_carries_each_credential_in_its_own_field() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let (account_id, refresh_ref) = registered_and_signed_in(&host, REFRESH_TOKEN).await;
    let mut renewal = request(
        vec![
            value("client_id", "{{username}}"),
            value("client_secret", &secret("realdebrid_client_secret")),
            value("code", &secret(&refresh_ref)),
            value("grant_type", GRANT_TYPE),
        ],
        vec![value("Accept", "application/json")],
        "",
    );

    expand(&host, &identity(account_id), &mut renewal)
        .await
        .expect("both references pass their gate");

    assert_eq!(field(&renewal.query, "client_id"), CLIENT_ID);
    assert_eq!(field(&renewal.query, "client_secret"), CLIENT_SECRET);
    assert_eq!(field(&renewal.query, "code"), REFRESH_TOKEN);
    assert_eq!(field(&renewal.query, "grant_type"), GRANT_TYPE);
    assert_eq!(occurrences(&renewal, CLIENT_SECRET), 1);
    assert_eq!(occurrences(&renewal, REFRESH_TOKEN), 1);
}

/// Spread over query, an allowed header and a JSON body, each canary lands exactly where its
/// reference was named and nowhere else.
#[tokio::test]
async fn each_canary_appears_only_where_its_reference_was_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let (account_id, refresh_ref) = registered_and_signed_in(&host, REFRESH_TOKEN).await;
    let a = secret("realdebrid_client_secret");
    let b = secret(&refresh_ref);
    let mut scattered = request(
        vec![value("first", &a), value("second", &b)],
        vec![
            value("content-type", "application/json"),
            value("Authorization", &format!("Bearer {b}")),
        ],
        &format!(r#"{{"a":"{a}","b":"{b}","both":"{a}|{b}"}}"#),
    );

    expand(&host, &identity(account_id), &mut scattered)
        .await
        .expect("expanded");

    assert_eq!(field(&scattered.query, "first"), CLIENT_SECRET);
    assert_eq!(field(&scattered.query, "second"), REFRESH_TOKEN);
    assert_eq!(
        field(&scattered.headers, "Authorization"),
        format!("Bearer {REFRESH_TOKEN}")
    );
    assert_eq!(
        String::from_utf8(scattered.body.clone()).expect("utf-8"),
        format!(
            r#"{{"a":"{CLIENT_SECRET}","b":"{REFRESH_TOKEN}","both":"{CLIENT_SECRET}|{REFRESH_TOKEN}"}}"#
        )
    );
    assert_eq!(
        occurrences(&scattered, CLIENT_SECRET),
        3,
        "query, body a, body both"
    );
    assert_eq!(
        occurrences(&scattered, REFRESH_TOKEN),
        4,
        "query, header, body b, body both"
    );
    assert!(
        wire(&scattered)
            .iter()
            .all(|text| !text.contains("{{secret:")),
        "no marker left behind"
    );
}

/// The same reference twice is one reference, loaded once and filled into both places.
#[tokio::test]
async fn the_same_reference_twice_is_filled_twice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let (account_id, _) = registered_and_signed_in(&host, REFRESH_TOKEN).await;
    let a = secret("realdebrid_client_secret");
    let mut twice = request(
        vec![value("one", &a)],
        vec![value("Authorization", &format!("Basic {a}:{a}"))],
        "",
    );
    assert_eq!(
        secret_references(&twice).expect("within the cap"),
        ["realdebrid_client_secret"]
    );

    expand(&host, &identity(account_id), &mut twice)
        .await
        .expect("expanded");

    assert_eq!(field(&twice.query, "one"), CLIENT_SECRET);
    assert_eq!(
        field(&twice.headers, "Authorization"),
        format!("Basic {CLIENT_SECRET}:{CLIENT_SECRET}")
    );
    assert_eq!(occurrences(&twice, REFRESH_TOKEN), 0);
}

/// Each reference passes its own gate, and one that fails refuses the whole request -- also
/// when it is the second, after the first already passed.
#[tokio::test]
async fn one_reference_failing_its_gate_refuses_the_whole_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let (account_id, _) = registered_and_signed_in(&host, REFRESH_TOKEN).await;
    let (_, foreign_refresh_ref) = registered_and_signed_in(&host, "CANARY-C-foreign").await;
    let mut request = request(
        vec![
            value("client_secret", &secret("realdebrid_client_secret")),
            value("code", &secret(&foreign_refresh_ref)),
        ],
        Vec::new(),
        "",
    );

    let failure = expand(&host, &identity(account_id), &mut request)
        .await
        .expect_err("another account's renewal material is not this request's");

    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
    assert_eq!(
        occurrences(&request, CLIENT_SECRET),
        0,
        "nothing was expanded"
    );
    assert_eq!(occurrences(&request, "CANARY-C-foreign"), 0);
}

/// More distinct references than the cap is refused before any of them is loaded: the first
/// would pass its gate, and the answer is still the cap's code rather than a value.
#[tokio::test]
async fn more_references_than_the_cap_are_refused_before_any_is_loaded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let (account_id, refresh_ref) = registered_and_signed_in(&host, REFRESH_TOKEN).await;
    let mut query = vec![
        value("a", &secret("realdebrid_client_secret")),
        value("b", &secret(&refresh_ref)),
    ];
    for extra in 0..MAX_SECRET_REFERENCES - 1 {
        query.push(value("x", &secret(&format!("realdebrid_extra_{extra}"))));
    }
    let request = request(query, Vec::new(), "");

    let failure = host
        .request_secrets(&identity(account_id), &request)
        .await
        .expect_err("over the cap");

    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_references_exceeded")
    );
    assert_eq!(failure.params.get("limit").map(String::as_str), Some("4"));
}

// -- the pure parts ----------------------------------------------------------

#[test]
fn every_marker_in_one_value_is_found_in_order() {
    let found: Vec<&str> = markers(
        "x{{secret:a}}y{{secret:b}}z{{secret:a}}",
        SECRET_MARKER_OPEN,
    )
    .collect();
    assert_eq!(found, ["a", "b", "a"]);
    // An unterminated marker names nothing, as it never did.
    assert_eq!(markers("{{secret:a", SECRET_MARKER_OPEN).count(), 0);
}

#[test]
fn references_are_distinct_across_kinds_and_places() {
    let mut request = request(
        vec![value("q", "{{secret:a}}{{secret:b}}")],
        vec![
            value("Authorization", "Basic {{basic:c}}"),
            value("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "{{secret:a}}&{{basic:b}}",
    );
    assert_eq!(
        secret_references(&request).expect("within"),
        ["a", "b", "c"]
    );
    request.body = b"{{secret:d}}{{secret:e}}".to_vec();
    let failure = secret_references(&request).expect_err("five is over the cap");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_references_exceeded")
    );
}

/// The heart of the defect, at the level where it lived: a marker naming a reference nothing
/// was loaded for is refused, never filled with the value loaded for a different one.
#[test]
fn a_marker_is_never_filled_with_another_references_value() {
    let mut secrets = Secrets::default();
    secrets.insert("a", SecretString::from(CLIENT_SECRET.to_owned()));
    let failure =
        expand_template("{{secret:b}}", &secrets, Escape::None).expect_err("b was not loaded");
    assert_eq!(failure.code.as_deref(), Some("plugin.secret_missing"));
    assert_eq!(
        expand_template("{{secret:a}}", &secrets, Escape::None).expect("a was"),
        CLIENT_SECRET
    );
}

/// A substituted value is not scanned again, so a secret that contains marker text cannot
/// pull a second credential into the request.
#[test]
fn a_substituted_value_is_not_expanded_a_second_time() {
    let mut secrets = Secrets::default();
    secrets.insert("a", SecretString::from("{{secret:b}}".to_owned()));
    secrets.insert("b", SecretString::from(REFRESH_TOKEN.to_owned()));
    assert_eq!(
        expand_template("{{secret:a}}", &secrets, Escape::None).expect("expanded"),
        "{{secret:b}}"
    );
}

/// A `Debug` of what was loaded names references and never a value.
#[test]
fn debug_output_carries_no_value() {
    let mut secrets = Secrets::default();
    secrets.insert("a", SecretString::from(CLIENT_SECRET.to_owned()));
    secrets.set_granted(SecretString::from(REFRESH_TOKEN.to_owned()));
    let printed = format!("{secrets:?}");
    assert!(!printed.contains(CLIENT_SECRET) && !printed.contains(REFRESH_TOKEN));
}
