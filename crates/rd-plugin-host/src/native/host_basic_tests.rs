//! The empty-user-name rule of `{{basic:…}}`, through the real `request_username` and
//! `expand_request` pipeline and the bundled provider rows (RD-120-38).
//!
//! Both directions are pinned against the two providers the rule exists for. Seedr requires a
//! user name -- its e-mail address is half the credential -- so an account without one is
//! refused before anything is built. Pixeldrain does not: its API key is the Basic password
//! under an empty name, so the same marker builds `base64(":key")` there.

use std::sync::Arc;

use rd_core::AccountId;
use rd_db::NewAccount;
use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostRequestValue};
use tokio::sync::RwLock;

use super::super::expand::expand_request;
use super::NativeHost;

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

async fn account(
    host: &NativeHost,
    provider: &str,
    username: Option<&str>,
    secret: &str,
) -> ClientIdentity {
    let secret_ref = host
        .secrets
        .put_string(secret.to_owned())
        .await
        .expect("put secret");
    let id: AccountId = host
        .database
        .create_account(NewAccount {
            provider: provider.to_owned(),
            label: "Test".to_owned(),
            username: username.map(str::to_owned),
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id;
    ClientIdentity {
        account_id: Some(id),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn basic_request(url: &str, reference: &str, body: &str) -> HostHttpRequest {
    HostHttpRequest {
        method: "GET".to_owned(),
        url: url.parse().expect("url"),
        query: Vec::new(),
        headers: vec![
            HostRequestValue {
                name: "Authorization".to_owned(),
                value_template: format!("Basic {{{{basic:{reference}}}}}"),
            },
            // A form, so a marker in the body is a template (RD-120-66).
            HostRequestValue {
                name: "Content-Type".to_owned(),
                value_template: "application/x-www-form-urlencoded".to_owned(),
            },
        ],
        body: body.as_bytes().to_vec(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    }
}

/// What `http_request` does before it sends: resolve both halves, then expand.
async fn expand(
    host: &NativeHost,
    identity: &ClientIdentity,
    request: &mut HostHttpRequest,
) -> Result<(), rd_core::Failure> {
    let secrets = host.request_secrets(identity, request).await?;
    let (username, optional) = host.request_username(identity, request).await?;
    expand_request(request, &secrets, username.as_deref(), None, optional).map(|_| ())
}

#[tokio::test]
async fn pixeldrain_builds_the_pair_under_an_empty_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let identity = account(&host, "pixeldrain", None, "api-key").await;
    let mut request = basic_request("https://pixeldrain.com/api/user", "pixeldrain_api_key", "");
    expand(&host, &identity, &mut request)
        .await
        .expect("an empty name is this provider's own shape");
    // base64(":api-key")
    assert_eq!(request.headers[0].value_template, "Basic OmFwaS1rZXk=");
}

#[tokio::test]
async fn seedr_refuses_an_account_without_its_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    for username in [None, Some("")] {
        let identity = account(&host, "seedr", username, "password").await;
        let mut request = basic_request("https://www.seedr.cc/rest/user", "seedr_password", "");
        let failure = expand(&host, &identity, &mut request)
            .await
            .expect_err("half a credential");
        assert_eq!(failure.code.as_deref(), Some("plugin.username_missing"));
    }
    // And with the name, the pair is built as before.
    let identity = account(&host, "seedr", Some("me@example.test"), "password").await;
    let mut request = basic_request("https://www.seedr.cc/rest/user", "seedr_password", "");
    expand(&host, &identity, &mut request)
        .await
        .expect("a complete account");
    // base64("me@example.test:password")
    assert_eq!(
        request.headers[0].value_template,
        "Basic bWVAZXhhbXBsZS50ZXN0OnBhc3N3b3Jk"
    );
}

/// The allowance is for the Basic pair alone. A bare `{{username}}` in the same request still
/// refuses an empty name, as it did before there was any allowance.
#[tokio::test]
async fn a_bare_username_marker_keeps_refusing_an_empty_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let identity = account(&host, "pixeldrain", None, "api-key").await;
    let mut request = basic_request(
        "https://pixeldrain.com/api/user",
        "pixeldrain_api_key",
        "user={{username}}",
    );
    let failure = expand(&host, &identity, &mut request)
        .await
        .expect_err("a bare marker needs a name");
    assert_eq!(failure.code.as_deref(), Some("plugin.username_missing"));
}

/// The key reaches `pixeldrain.com` and not the delivery sub-domain the manifest allows for
/// downloads: the credential's hosts are its slot's, not the sandbox's.
#[tokio::test]
async fn the_pixeldrain_key_stays_on_its_own_host() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let identity = account(&host, "pixeldrain", None, "api-key").await;
    let mut request = basic_request(
        "https://cdn.pixeldrain.com/api/file/abc",
        "pixeldrain_api_key",
        "",
    );
    let failure = expand(&host, &identity, &mut request)
        .await
        .expect_err("not a secret domain");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}
