//! Named parts a sign-in keeps beside its token (RD-150-09), through the real `NativeHost`.
//!
//! Real-Debrid's open-source device flow hands every person a client id and a client secret of
//! their own, and every renewal needs both beside the refresh material. These tests hold the
//! contract the host makes about them: each part is kept on its own and filled only into the
//! marker that names it, it reaches `api.real-debrid.com` and nowhere else, only the account's
//! own provider can name it and only in the sign-in mode, and nothing reads it back. The values
//! are canaries, so a test can say not only where each landed but that it is nowhere else.

use std::sync::Arc;

use rd_core::AccountId;
use rd_db::NewAccount;
use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostRequestValue, ResolverHost};
use rd_provider_registry::CredentialMode;
use tokio::sync::RwLock;

use super::super::expand::expand_request;
use super::NativeHost;

const CLIENT_ID: &str = "CANARY-P-personal-client-id-31aa";
const CLIENT_SECRET: &str = "CANARY-Q-personal-client-secret-94cd";
const TOKEN_ENDPOINT: &str = "https://api.real-debrid.com/oauth/v2/token";

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

/// A Real-Debrid account in `mode`, with nothing typed: what the accounts form creates for
/// "Connect with a code".
async fn real_debrid_account(host: &NativeHost, mode: CredentialMode) -> AccountId {
    host.database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: None,
            credential_mode: Some(mode),
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id
}

fn identity(account_id: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account_id),
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

fn value(name: &str, template: &str) -> HostRequestValue {
    HostRequestValue {
        name: name.to_owned(),
        value_template: template.to_owned(),
    }
}

/// The token exchange `realdebrid-auth` sends, naming both parts.
fn exchange(url: &str) -> HostHttpRequest {
    HostHttpRequest {
        method: "POST".to_owned(),
        url: url.parse().expect("url"),
        query: vec![
            value("client_id", "{{secret:realdebrid_client_id}}"),
            value("client_secret", "{{secret:realdebrid_client_secret}}"),
            value("code", "a-device-code"),
        ],
        headers: vec![value("Accept", "application/json")],
        body: Vec::new(),
        granted_secret: None,
        authority: rd_plugin_api::RequestAuthority::Provider,
        write_methods: false,
    }
}

fn field<'a>(request: &'a HostHttpRequest, name: &str) -> &'a str {
    request
        .query
        .iter()
        .find(|value| value.name == name)
        .map(|value| value.value_template.as_str())
        .expect("field present")
}

/// What `http_request` does before it sends: resolve every credential, then expand.
async fn expand(
    host: &NativeHost,
    account_id: AccountId,
    request: &mut HostHttpRequest,
) -> Result<(), rd_core::Failure> {
    let secrets = host.request_secrets(&identity(account_id), request).await?;
    expand_request(request, &secrets, None, None, false).map(|_| ())
}

#[tokio::test]
async fn each_part_is_kept_on_its_own_and_filled_only_where_it_is_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;

    // Nothing kept yet: the account reads as not holding a client.
    assert!(
        !host
            .secret_available(account, "realdebrid_client_secret")
            .await
    );

    host.store_flow_secret(account, "realdebrid_client_id", CLIENT_ID)
        .await
        .expect("the client id is a part");
    host.store_flow_secret(account, "realdebrid_client_secret", CLIENT_SECRET)
        .await
        .expect("the client secret is a part");

    // Available, and still not the token: a kept client is not a signed-in account.
    assert!(host.secret_available(account, "realdebrid_client_id").await);
    assert!(
        host.secret_available(account, "realdebrid_client_secret")
            .await
    );
    assert!(
        !host
            .secret_available(account, "realdebrid_access_token")
            .await
    );

    // Two vault entries, one per part, never one joined value.
    let id_ref = host
        .database
        .auth_flow_part(account, "realdebrid_client_id")
        .await
        .expect("read")
        .expect("stored");
    let secret_ref = host
        .database
        .auth_flow_part(account, "realdebrid_client_secret")
        .await
        .expect("read")
        .expect("stored");
    assert_ne!(id_ref, secret_ref);

    let mut request = exchange(TOKEN_ENDPOINT);
    expand(&host, account, &mut request)
        .await
        .expect("both parts pass their gate");
    assert_eq!(field(&request, "client_id"), CLIENT_ID);
    assert_eq!(field(&request, "client_secret"), CLIENT_SECRET);
    assert_eq!(field(&request, "code"), "a-device-code");
}

/// The exchange as `realdebrid-auth` sends it since 1.5.2: every field in a form body, which
/// is the only place Real-Debrid's token endpoint reads them from. Each part lands in its own
/// field, percent-encoded, so a value holding the form's separators cannot add a field.
#[tokio::test]
async fn the_parts_fill_a_form_body_encoded_for_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;
    let secret = "CANARY-Q&grant_type=other +/";
    host.store_flow_secret(account, "realdebrid_client_id", CLIENT_ID)
        .await
        .expect("stored");
    host.store_flow_secret(account, "realdebrid_client_secret", secret)
        .await
        .expect("stored");

    let mut request = exchange(TOKEN_ENDPOINT);
    request.query.clear();
    request
        .headers
        .push(value("Content-Type", "application/x-www-form-urlencoded"));
    request.body = b"client_id={{secret:realdebrid_client_id}}\
        &client_secret={{secret:realdebrid_client_secret}}\
        &code=a-device-code\
        &grant_type=http%3A%2F%2Foauth.net%2Fgrant_type%2Fdevice%2F1.0"
        .to_vec();
    expand(&host, account, &mut request)
        .await
        .expect("both parts pass their gate in a body");

    let form: Vec<(String, String)> = url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect();
    assert_eq!(
        form,
        vec![
            ("client_id".to_owned(), CLIENT_ID.to_owned()),
            ("client_secret".to_owned(), secret.to_owned()),
            ("code".to_owned(), "a-device-code".to_owned()),
            (
                "grant_type".to_owned(),
                "http://oauth.net/grant_type/device/1.0".to_owned()
            ),
        ]
    );
}

#[tokio::test]
async fn a_new_sign_in_replaces_a_part_and_drops_the_old_value() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;
    host.store_flow_secret(account, "realdebrid_client_secret", "CANARY-old-secret")
        .await
        .expect("stored");
    let old = host
        .database
        .auth_flow_part(account, "realdebrid_client_secret")
        .await
        .expect("read")
        .expect("stored");

    host.store_flow_secret(account, "realdebrid_client_secret", CLIENT_SECRET)
        .await
        .expect("replaced");

    let new = host
        .database
        .auth_flow_part(account, "realdebrid_client_secret")
        .await
        .expect("read")
        .expect("stored");
    assert_ne!(old, new);
    assert!(
        host.secrets.get(&old).await.is_err(),
        "the replaced value is gone from the vault"
    );
}

#[tokio::test]
async fn a_part_reaches_the_provider_s_api_and_nowhere_else() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;
    host.store_flow_secret(account, "realdebrid_client_secret", CLIENT_SECRET)
        .await
        .expect("stored");
    host.store_flow_secret(account, "realdebrid_client_id", CLIENT_ID)
        .await
        .expect("stored");

    // The sign-in page is in the plugin's domains, and not in the slot's.
    for elsewhere in [
        "https://real-debrid.com/device",
        "https://www.real-debrid.com/oauth/v2/token",
    ] {
        let mut request = exchange(elsewhere);
        let failure = expand(&host, account, &mut request)
            .await
            .expect_err("a part may not leave api.real-debrid.com");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.secret_target_not_allowed"),
            "{elsewhere}"
        );
        assert!(
            request
                .query
                .iter()
                .all(|value| !value.value_template.contains("CANARY")),
            "nothing was expanded for {elsewhere}"
        );
    }
}

#[tokio::test]
async fn only_the_sign_in_mode_keeps_or_reaches_parts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::ApiKey).await;

    let failure = host
        .store_flow_secret(account, "realdebrid_client_secret", CLIENT_SECRET)
        .await
        .expect_err("an account holding a typed key keeps no sign-in parts");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.store_token_not_allowed")
    );

    // Even a part that somehow exists is out of reach in the other mode.
    let stored = host
        .secrets
        .put_string(CLIENT_SECRET.to_owned())
        .await
        .expect("put");
    host.database
        .set_auth_flow_part(account, "realdebrid_client_secret".to_owned(), stored)
        .await
        .expect("row");
    assert!(
        !host
            .secret_available(account, "realdebrid_client_secret")
            .await
    );
    let mut request = exchange(TOKEN_ENDPOINT);
    request.query.retain(|value| value.name != "client_id");
    let failure = expand(&host, account, &mut request)
        .await
        .expect_err("not in this mode");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.secret_target_not_allowed")
    );
}

#[tokio::test]
async fn only_a_declared_part_can_be_written() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;
    for name in [
        // The token's slot is written by `store-oauth-token`, never through this call.
        "realdebrid_access_token",
        // What the person types in the other mode.
        "realdebrid_api_token",
        // Another provider's slot, and a name nobody declared.
        "oauthapp_client_secret",
        "realdebrid_anything",
    ] {
        let failure = host
            .store_flow_secret(account, name, CLIENT_SECRET)
            .await
            .expect_err(name);
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.store_token_not_allowed"),
            "{name}"
        );
    }
    let failure = host
        .store_flow_secret(account, "realdebrid_client_secret", "  ")
        .await
        .expect_err("an empty part is no part");
    assert_eq!(failure.code.as_deref(), Some("plugin.store_token_empty"));
}

#[tokio::test]
async fn a_cancelled_sign_in_takes_its_parts_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let host = test_host(directory.path()).await;
    let account = real_debrid_account(&host, CredentialMode::OAuth).await;
    host.store_flow_secret(account, "realdebrid_client_secret", CLIENT_SECRET)
        .await
        .expect("stored");

    host.database
        .delete_auth_flow(account)
        .await
        .expect("cancel");

    assert!(
        host.database
            .auth_flow_part(account, "realdebrid_client_secret")
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        !host
            .secret_available(account, "realdebrid_client_secret")
            .await
    );
}
