//! The sign-in half of the storage harness: the cases the cloud drives' OAuth plugins answer
//! alike, against the mock token endpoint in `endpoint`. They share `plugin-guest-oauth`'s
//! flow, so what differs is data: endpoints, scope, whether PKCE or a client secret binds the
//! exchange, and how long a token lasts.

use std::sync::Arc;

use rd_core::{AccountId, FailureKind};
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginManifest,
    extension::{OAuthProvider, TokenOutcome},
};

use super::marker;
use crate::common::oauth::{Stored, parameter, s256};

mod endpoint;

pub use endpoint::TokenEndpoint;

pub const PLACEHOLDER_ACCESS_TOKEN: &str = "redacted-access-token-0000";
pub const PLACEHOLDER_REFRESH_TOKEN: &str = "redacted-refresh-token-0000";

/// What the mock token endpoint answers next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Checks the PKCE verifier, where the row has one, and hands the tokens over.
    Granted,
    /// A renewal: no PKCE, and the provider's own renewal answer.
    RenewalGranted,
    Denied,
    InvalidGrant,
    /// A client secret the provider does not accept; Box's case.
    InvalidClient,
    RateLimited,
    /// The provider could not be reached at all: not an answer, a failed call.
    Unreachable,
    /// The device endpoint hands out a code and the token endpoint says "not yet".
    DevicePending,
    /// The device endpoint hands out a code and the token endpoint grants.
    DeviceGranted,
    /// The device endpoint hands out a code and the person declines at the other screen.
    DeviceDeclined,
}

/// The sanitised token answers, one fixture each.
#[derive(Clone, Copy)]
pub struct Tokens {
    pub granted: &'static str,
    pub renewed: &'static str,
    pub denied: &'static str,
    pub invalid_grant: &'static str,
    pub invalid_client: Option<&'static str>,
    pub rate_limited: &'static str,
    pub device: Option<DeviceAnswers>,
}

/// The device entrance's answers, for a provider that offers one.
#[derive(Clone, Copy)]
pub struct DeviceAnswers {
    pub code: &'static str,
    pub pending: &'static str,
    pub declined: &'static str,
}

/// One provider's sign-in, as a row of the table.
pub struct SignIn {
    pub component: &'static str,
    pub manifest: &'static str,
    pub tokens: Tokens,
    /// The address the person is sent to, up to its query.
    pub authorize: &'static str,
    pub token_endpoint: &'static str,
    /// The vault reference the token is stored under and renewed from.
    pub access_reference: &'static str,
    /// A confidential client's secret, sent as a marker on every grant.
    pub client_secret: Option<&'static str>,
    pub pkce: bool,
    /// What the authorization address carries beyond `response_type`, `state` and the client.
    pub authorize_parameters: &'static [(&'static str, &'static str)],
    /// What a renewal carries beyond its grant type, the refresh marker and the client.
    pub renewal_fields: &'static [(&'static str, &'static str)],
    pub expires_in_seconds: u64,
    /// Whether a renewal answer carries refresh material: Microsoft rotates it and Box sends it
    /// again; Dropbox and Google issue it once and expect it to keep working.
    pub renewal_keeps_refresh: bool,
    /// The refusals of the token endpoint and the provider's code each has to keep.
    pub refusals: &'static [(Case, &'static str)],
    /// The sentences beside those codes, none of which may reach a message.
    pub prose: &'static [&'static str],
}

/// The sign-in of `row`, compiled, in front of `host`.
pub fn plugin(row: &SignIn, bytes: &[u8], host: Arc<TokenEndpoint>) -> OAuthProvider {
    let manifest: PluginManifest = toml::from_str(row.manifest).expect("the oauth manifest");
    let host: Arc<dyn ResolverHost> = host;
    OAuthProvider::new(manifest, bytes, Some(host)).expect("the sign-in plugin builds")
}

fn component(row: &SignIn) -> Vec<u8> {
    rd_plugin_host::artifact::component(row.component)
}

/// What a grant stores: the placeholders, with or without the refresh half.
fn stored(row: &SignIn, account_id: AccountId, refresh: bool) -> Vec<Stored> {
    vec![Stored {
        account_id,
        access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
        refresh_token: refresh.then(|| PLACEHOLDER_REFRESH_TOKEN.to_owned()),
        expires_in_seconds: Some(row.expires_in_seconds),
    }]
}

/// The browser sign-in, from the address the person is sent to all the way to the stored token
/// and its renewal material, with no part of the application's registration in the plugin: the
/// marker travels and the host substitutes what *this installation* registered (RD-106-04).
pub async fn the_browser_sign_in_runs_through(row: &SignIn) {
    let bytes = component(row);
    let server = TokenEndpoint::new(row, Case::Granted);
    let plugin = plugin(row, &bytes, server.clone());
    let account = AccountId::new();

    let request = plugin.begin(account, None).await.expect("begin");
    let address = &request.authorization_url;
    assert!(address.starts_with(row.authorize), "{address}");
    assert_eq!(
        parameter(&request, "response_type").as_deref(),
        Some("code")
    );
    for (name, value) in row.authorize_parameters {
        assert_eq!(parameter(&request, name).as_deref(), Some(*value), "{name}");
    }
    assert!(
        address.contains("client_id=%7B%7Bclient_id%7D%7D")
            || address.contains("client_id={{client_id}}"),
        "the authorization URL must carry the marker, not a client id: {address}"
    );
    // The address the person is sent to is one the manifest declares, which is the gate the host
    // applies before showing it.
    let manifest: PluginManifest = toml::from_str(row.manifest).expect("the oauth manifest");
    assert!(rd_plugin_host::domain_allowed(
        &url::Url::parse(address).expect("URL"),
        manifest.domains()
    ));
    assert_eq!(
        parameter(&request, "state").as_deref(),
        Some(request.state.as_str())
    );
    if row.pkce {
        assert_eq!(
            parameter(&request, "code_challenge_method").as_deref(),
            Some("S256")
        );
        let challenge = parameter(&request, "code_challenge").expect("a challenge");
        let verifier = request.flow_state.clone().expect("a verifier");
        assert!(
            !address.contains(&verifier),
            "the verifier must never appear in the address the person is sent to"
        );
        assert_eq!(s256(&verifier), challenge);
        assert!((43..=128).contains(&verifier.len()));
        server.expect_challenge(&challenge);
    } else {
        // Nothing to carry from `begin` to `poll`, and nothing invented to look like it.
        assert_eq!(request.flow_state, None);
    }

    let outcome = plugin
        .poll(
            account,
            "the-code-the-callback-carried",
            request.flow_state.as_deref(),
        )
        .await
        .expect("poll");
    assert_eq!(outcome, TokenOutcome::Authorized);

    // The mock stands where the host's expansion would be, so what it sees is what the plugin
    // wrote: the markers, and never a value.
    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.url, row.token_endpoint);
    assert_eq!(exchange.field("grant_type"), Some("authorization_code"));
    assert_eq!(
        exchange.field("code_verifier"),
        request.flow_state.as_deref()
    );
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    // A public client sends no secret, because there is nowhere it could live.
    assert_eq!(
        exchange.field("client_secret").map(str::to_owned),
        row.client_secret.map(marker)
    );
    assert_eq!(server.stored(), stored(row, account, true));
}

/// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
pub async fn a_code_without_its_verifier_is_refused(row: &SignIn) {
    let bytes = component(row);
    let server = TokenEndpoint::new(row, Case::Granted);
    let plugin = plugin(row, &bytes, server.clone());
    let account = AccountId::new();
    let request = plugin.begin(account, None).await.expect("begin");
    server.expect_challenge(&parameter(&request, "code_challenge").expect("challenge"));

    let outcome = plugin
        .poll(
            account,
            "a-stolen-code",
            Some("a-verifier-from-another-flow"),
        )
        .await
        .expect("poll");
    assert!(
        matches!(outcome, TokenOutcome::Failed { .. }),
        "{outcome:?}"
    );
    assert!(server.stored().is_empty(), "nothing may be stored");
}

/// Two flows started in the same moment, for the same account, share nothing.
pub async fn two_flows_share_nothing(row: &SignIn) {
    let bytes = component(row);
    let plugin = plugin(row, &bytes, TokenEndpoint::new(row, Case::Granted));
    let account = AccountId::new();

    let first = plugin.begin(account, None).await.expect("begin");
    let second = plugin.begin(account, None).await.expect("begin");
    assert_ne!(first.state, second.state, "the state repeated");
    if row.pkce {
        assert_ne!(first.flow_state, second.flow_state, "the verifier repeated");
        assert_ne!(
            Some(first.state.clone()),
            first.flow_state,
            "the state and the verifier are the same value"
        );
    }
    for request in [&first, &second] {
        assert!(!request.state.contains(&account.to_string()));
    }
}

/// Each refusal ends the flow with its own code and as a refused credential, which the renewal
/// sweep gives up rather than holds (RD-106-02); a provider that cannot be reached ends
/// nothing, or people would be signed out whenever a connection drops.
pub async fn a_refusal_ends_the_flow_and_an_outage_does_not(row: &SignIn) {
    let bytes = component(row);
    let account = AccountId::new();
    for (case, expected) in row.refusals {
        let server = TokenEndpoint::new(row, *case);
        let outcome = plugin(row, &bytes, server.clone())
            .refresh(account, Some(row.access_reference))
            .await
            .expect("refresh");
        let TokenOutcome::Failed { category, message } = outcome else {
            panic!("{case:?} should have failed, got {outcome:?}");
        };
        // The provider's own error code survives, because it is shaped like one; the sentence
        // beside it does not.
        assert!(message.contains(*expected), "{case:?}: {message}");
        for sentence in row.prose {
            assert!(
                !message.contains(*sentence),
                "the provider's prose reached the message: {message}"
            );
        }
        assert_eq!(category, FailureKind::AuthRequired, "{case:?}");
        assert!(server.stored().is_empty());
    }

    let offline = TokenEndpoint::new(row, Case::Unreachable);
    let outcome = plugin(row, &bytes, offline.clone())
        .refresh(account, Some(row.access_reference))
        .await;
    assert!(
        outcome.is_err(),
        "an unreachable provider must not end the flow"
    );
    assert!(offline.stored().is_empty());
}

/// A renewal stores the new token without anybody being asked, sends the stored refresh
/// material as a template rather than a value, and never carries a code.
pub async fn a_renewal_stores_a_new_token_without_a_redirect(row: &SignIn) {
    let bytes = component(row);
    let server = TokenEndpoint::new(row, Case::RenewalGranted);
    let account = AccountId::new();
    let outcome = plugin(row, &bytes, server.clone())
        .refresh(account, Some(row.access_reference))
        .await
        .expect("refresh");
    assert_eq!(outcome, TokenOutcome::Authorized);

    let sent = server.requests().pop().expect("one request");
    assert_eq!(sent.url, row.token_endpoint);
    assert_eq!(sent.field("grant_type"), Some("refresh_token"));
    assert_eq!(
        sent.field("refresh_token"),
        Some(marker(row.access_reference).as_str())
    );
    assert_eq!(sent.field("client_id"), Some("{{client_id}}"));
    assert_eq!(
        sent.field("client_secret").map(str::to_owned),
        row.client_secret.map(marker)
    );
    for (name, value) in row.renewal_fields {
        assert_eq!(sent.field(name), Some(*value), "{name}");
    }
    assert!(sent.field("code").is_none(), "a renewal carries no code");
    assert!(
        sent.field("device_code").is_none(),
        "a renewal is not a device sign-in"
    );
    assert_eq!(
        server.stored(),
        stored(row, account, row.renewal_keeps_refresh)
    );
}

/// A rate limit is a wait, not a failure, and carries the provider's own `Retry-After`.
pub async fn a_rate_limit_becomes_a_wait(row: &SignIn) {
    let bytes = component(row);
    let server = TokenEndpoint::new(row, Case::RateLimited);
    let renewed = plugin(row, &bytes, server.clone())
        .refresh(AccountId::new(), Some(row.access_reference))
        .await
        .expect("refresh");
    assert_eq!(
        renewed,
        TokenOutcome::Pending {
            retry_after_seconds: 90
        }
    );
    assert!(server.stored().is_empty(), "a rate limit stores nothing");
}

/// A provider without a device entrance refuses one with a stable code rather than a trap, and
/// asks the provider nothing.
pub async fn the_device_entrance_is_refused(row: &SignIn) {
    let bytes = component(row);
    let server = TokenEndpoint::new(row, Case::Granted);
    let failure = plugin(row, &bytes, server.clone())
        .device_begin(AccountId::new(), None)
        .await
        .expect_err("no device entrance");
    let shown = format!("{failure}");
    assert!(
        shown.contains("flow_unsupported") || format!("{failure:?}").contains("flow_unsupported"),
        "{failure:?}"
    );
    assert!(shown.contains("device code"), "{shown}");
    assert!(
        server.requests().is_empty(),
        "a flow that is not offered must ask the provider nothing"
    );
}
