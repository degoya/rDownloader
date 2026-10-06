//! The account-less flow's edges: a resolve after a restart, an account on the request, and a
//! link refused before any request. Split from `flow_tests.rs` to keep both files under the
//! crate layout's 500 lines.

use rd_core::FailureKind;
use rd_plugin_api::{ClientIdentity, ResolveRequest, Resolver};

use super::super::KrakenfilesResolver;
use super::{
    CANONICAL_LINK, DIRECT_LINK, DOWNLOAD_OK, FILE_PAGE, MockHost, file, html, json,
    resolve_request,
};

/// Nothing survives a restart: a second resolve of the same link, on a fresh process, starts
/// at the page again and earns its own token and challenge.
#[tokio::test]
async fn a_resolve_after_a_restart_starts_from_the_page_again() {
    for _restart in 0..2 {
        let host = MockHost::free(
            vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
            Some("turnstile-token"),
        );
        let resolver = KrakenfilesResolver::new(host.clone());
        resolver
            .resolve(resolve_request(CANONICAL_LINK))
            .await
            .expect("resolves");
        assert_eq!(host.request_count(), 3);
        assert_eq!(host.captcha_count(), 1);
    }
}

/// An account on the request cannot belong to this provider; the flow is the same.
#[tokio::test]
async fn an_account_on_the_request_is_ignored() {
    let host = MockHost::free(
        vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let resolved = resolver
        .resolve(ResolveRequest {
            url: CANONICAL_LINK.parse().expect("url"),
            client: ClientIdentity {
                account_id: Some(rd_core::AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 3,
            },
        })
        .await
        .expect("resolves");

    assert_eq!(resolved.url.as_str(), DIRECT_LINK);
    assert_eq!(
        resolved.client.tls_revision, 3,
        "the identity comes back untouched"
    );
}

/// An unsupported link is refused before any request is made.
#[tokio::test]
async fn an_unsupported_link_is_refused_without_a_request() {
    let host = MockHost::free(Vec::new(), Some("unused"));
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request("https://krakenfiles.com/view/DP3nGKJNsX"))
        .await
        .expect_err("short form");

    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.unsupported_link")
    );
    assert_eq!(host.request_count(), 0);
}
