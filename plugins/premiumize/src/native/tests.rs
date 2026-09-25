//! Native-adapter coverage for Premiumize.
//!
//! Drives the real resolver against queued mock responses, so it is the regression check
//! for the shared logic reached through the native adapter.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver, ResolverHost,
};

use super::PremiumizeResolver;

struct MockHost(Mutex<Option<HostHttpResponse>>);

#[async_trait]
impl ResolverHost for MockHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        assert_eq!(request.url.path(), "/api/transfer/directdl");
        assert!(!String::from_utf8_lossy(&request.body).contains("api-key"));
        self.0
            .lock()
            .expect("mock lock")
            .take()
            .ok_or_else(|| Failure::new(rd_core::FailureKind::Permanent, "missing mock response"))
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "premiumize_api_key"
    }
}

/// Parses an `account/info` payload the way `check_account` does and renders its label.
#[tokio::test]
async fn direct_download_keeps_client_identity() {
    let response = HostHttpResponse {
    status: 200,
    final_url: "https://www.premiumize.me/api/transfer/directdl"
        .parse()
        .expect("URL"),
    headers: Vec::new(),
    body: br#"{"status":"success","content":[{"path":"Folder/release.bin","size":42,"link":"https://cdn.premiumize.me/release.bin"}]}"#.to_vec(),
};
    let resolver = PremiumizeResolver::new(Arc::new(MockHost(Mutex::new(Some(response)))));
    let account = AccountId::new();
    let resolved = resolver
        .resolve(ResolveRequest {
            url: "https://example.test/source".parse().expect("URL"),
            client: ClientIdentity {
                account_id: Some(account),
                proxy_profile_id: None,
                tls_revision: 7,
            },
        })
        .await
        .expect("resolved");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(42));
    assert_eq!(resolved.client.tls_revision, 7);
}
