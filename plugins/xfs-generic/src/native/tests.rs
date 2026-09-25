//! The native adapter's contract, and what an empty domain list means for it.
//!
//! While `match_domains` is empty this plugin claims nothing — which is the state to assert, not
//! to work around. The free flow itself is tested in `src/resolver/free/tests.rs`, which drives
//! it directly and therefore keeps working the moment a verified clone is added.

use std::sync::Arc;

use rd_core::{AccountId, FailureKind};
use rd_plugin_api::{CheckRequest, ClientIdentity, Resolver, ResolverHost};
use url::Url;

use super::XfsGenericResolver;

/// A host that refuses everything, the way the conformance run's own host does. Nothing in this
/// file may need it to answer.
struct RefusingHost;

#[async_trait::async_trait]
impl ResolverHost for RefusingHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: rd_plugin_api::HostHttpRequest,
    ) -> Result<rd_plugin_api::HostHttpResponse, rd_core::Failure> {
        panic!("no request may be made here")
    }

    async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
        panic!("this plugin has no account")
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }

    async fn wait(&self, _client: &ClientIdentity, _seconds: u32) -> Result<(), rd_core::Failure> {
        panic!("no wait may happen here")
    }

    async fn solve_captcha(
        &self,
        _client: &ClientIdentity,
        _challenge: rd_plugin_api::CaptchaChallenge,
        _limit: std::time::Duration,
    ) -> Result<rd_plugin_api::CaptchaAnswer, rd_core::Failure> {
        panic!("no captcha may be solved here")
    }
}

fn resolver() -> XfsGenericResolver {
    XfsGenericResolver::new(Arc::new(RefusingHost) as Arc<dyn ResolverHost>)
}

fn client() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// The conformance check's own probes, asserted here so a change to `matches()` is caught long
/// before a package is built.
#[test]
fn no_foreign_link_is_claimed() {
    let resolver = resolver();
    for foreign in [
        "https://conformance.invalid/some/file.bin",
        "https://cdn.example.org/a/b/c.zip",
        "https://clone.test/abc123xyz/release.rar",
    ] {
        assert!(
            !resolver.matches(&foreign.parse().expect("URL")),
            "{foreign}"
        );
    }
}

/// The host list is the single authority for what is claimed: `hosters()` reports exactly it,
/// and `matches()` accepts exactly its members and nothing else.
///
/// This replaced an assertion that the list was *empty*, which held only while it was — the
/// invariant is what mattered, not the value.
#[tokio::test]
async fn the_host_list_is_the_only_authority_for_what_is_claimed() {
    let hosters = resolver()
        .hosters(AccountId::new())
        .await
        .expect("hosters answers");
    assert_eq!(hosters, crate::HOSTERS, "hosters() must report the list");
    for host in crate::HOSTERS {
        let url = format!("https://{host}/abc123xyz/release.rar");
        assert!(
            resolver().matches(&url.parse().expect("URL")),
            "a claimed host must match: {url}"
        );
        // The same site under `www.`, which every XFS installation serves.
        let www = format!("https://www.{host}/abc123xyz/release.rar");
        assert!(
            resolver().matches(&www.parse().expect("URL")),
            "a claimed host must match under www: {www}"
        );
    }
}

/// There is no account, and saying so is better than reporting a valid one that does nothing.
#[tokio::test]
async fn checking_an_account_reports_that_there_is_none() {
    let failure = resolver()
        .check_account(AccountId::new())
        .await
        .expect_err("no account to check");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("xfs_generic.no_account"));
}

/// A link check needs the account API this plugin does not have.
#[tokio::test]
async fn checking_a_link_is_unsupported() {
    let failure = resolver()
        .check(CheckRequest {
            urls: vec!["https://clone.test/abc123xyz".parse().expect("URL")],
            client: client(),
        })
        .await
        .expect_err("no link check");
    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(failure.code.as_deref(), Some("link.check_unsupported"));
}
