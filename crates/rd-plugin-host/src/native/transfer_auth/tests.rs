//! Which hosts a transfer may carry the account's credential to (RD-120-38). The redirect case
//! is a question about the address the bytes finally come from, which is why every assertion
//! below is about a target rather than about a source.

use rd_provider_registry::{
    CredentialKind, ProviderKind, ProviderSource, ProviderSpec, SecretFilledBy, SecretSlot,
    TransferAuth,
};
use url::Url;

use super::{basic_allowed, download_authorization};

fn url(value: &str) -> Url {
    Url::parse(value).expect("url")
}

/// A Seedr-shaped row, built by hand so no process-wide table takes part.
fn basic_provider(username_required: bool, domains: &[&str]) -> ProviderSpec {
    ProviderSpec {
        slug: "seedr".to_owned(),
        display_name: "Seedr".to_owned(),
        kind: ProviderKind::Hoster,
        credentials: if username_required {
            CredentialKind::UsernamePassword
        } else {
            CredentialKind::ApiKey
        },
        username_required,
        transfer_auth: TransferAuth::Basic,
        secrets: vec![SecretSlot {
            reference: "seedr_password".to_owned(),
            domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
            mode: None,
            filled_by: SecretFilledBy::Person,
        }],
        request_domains: vec!["www.seedr.cc".to_owned()],
        cookie_scope: None,
        match_hosts: vec!["www.seedr.cc".to_owned()],
        host_aliases: Vec::new(),
        source: ProviderSource::Plugin,
        plugin_id: None,
        plugin_version: None,
    }
}

#[test]
fn a_basic_credential_rides_only_to_the_hosts_its_slot_names() {
    let spec = basic_provider(true, &["www.seedr.cc"]);
    assert_eq!(
        download_authorization(
            &spec,
            &url("https://www.seedr.cc/rest/file/42"),
            Some("user"),
            "password"
        )
        .expect("a pair"),
        // base64("user:password")
        Some("Basic dXNlcjpwYXNzd29yZA==".to_owned())
    );
    // Seedr's own storage host is a sibling, not the host the credential was declared for; a
    // foreign CDN, a look-alike and a cleartext address are further off still.
    for elsewhere in [
        "https://d1.seedr.cc/ff/file.mkv",
        "https://cdn.foreign.test/file.mkv",
        "https://www.seedr.cc.evil.test/rest/file/42",
        "http://www.seedr.cc/rest/file/42",
    ] {
        assert!(!basic_allowed(&spec, &url(elsewhere)), "{elsewhere}");
        assert_eq!(
            download_authorization(&spec, &url(elsewhere), Some("user"), "password")
                .expect("no refusal"),
            None,
            "{elsewhere}"
        );
    }
}

/// A row that declares nothing keeps the arrangement every provider had before: the transfer
/// carries nothing the account holds, whatever the host.
#[test]
fn a_row_without_transfer_auth_sends_nothing() {
    let mut spec = basic_provider(true, &["www.seedr.cc"]);
    spec.transfer_auth = TransferAuth::None;
    assert_eq!(
        download_authorization(
            &spec,
            &url("https://www.seedr.cc/rest/file/42"),
            Some("user"),
            "password"
        )
        .expect("no refusal"),
        None
    );
    // Nor does a kind whose secret is not the credential, whatever the row claims.
    let mut cookies = basic_provider(true, &["www.seedr.cc"]);
    cookies.credentials = CredentialKind::ApiKeyOrCookies;
    assert!(!basic_allowed(
        &cookies,
        &url("https://www.seedr.cc/rest/file/42")
    ));
}

/// Both directions of the empty-name rule, on the transfer path: required means refused,
/// not required means `base64(":secret")` — Pixeldrain's shape.
#[test]
fn an_empty_name_is_a_refusal_unless_the_row_does_not_require_one() {
    let target = url("https://www.seedr.cc/rest/file/42");
    for username in [None, Some("")] {
        let failure = download_authorization(
            &basic_provider(true, &["www.seedr.cc"]),
            &target,
            username,
            "password",
        )
        .expect_err("half a credential");
        assert_eq!(failure.code.as_deref(), Some("plugin.username_missing"));
        assert!(!failure.message.contains("password"), "{}", failure.message);

        assert_eq!(
            download_authorization(
                &basic_provider(false, &["www.seedr.cc"]),
                &target,
                username,
                "api-key"
            )
            .expect("an empty name is allowed here"),
            Some("Basic OmFwaS1rZXk=".to_owned())
        );
    }
}

/// The OAuth path is unchanged by the second shape.
#[test]
fn an_oauth_row_still_gets_its_bearer_token() {
    let mut spec = basic_provider(false, &["www.googleapis.com"]);
    spec.credentials = CredentialKind::OAuth;
    spec.transfer_auth = TransferAuth::None;
    assert_eq!(
        download_authorization(
            &spec,
            &url("https://www.googleapis.com/drive/v3/files/a"),
            None,
            "token"
        )
        .expect("no refusal"),
        Some("Bearer token".to_owned())
    );
}
