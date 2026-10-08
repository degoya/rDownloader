//! The account check, the link check and the download host, against the two-installation mock
//! in `tests.rs`; split from it to keep both files under the crate layout's 500 lines.

use std::sync::Arc;

use async_trait::async_trait;
use pcloud_common::address::Region;
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};
use rd_plugin_types::{AccountId, Failure};

use super::super::PCloudResolver;
use super::{MockPCloud, OWN_FILE_US, client};

/// The account row says which installation the account lives in, because every region mistake
/// reads like a bad credential until somebody can see it.
#[tokio::test]
async fn the_account_label_names_the_installation_that_answered() {
    let host = MockPCloud::in_region(Region::Eu);
    let account = AccountId::new();
    let status = PCloudResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>)
        .check_account(account)
        .await
        .expect("an account");
    assert!(status.valid && status.premium);
    assert!(
        status
            .label
            .iter()
            .any(|part| part.code == "pcloud.account_region"
                && part.params.iter().any(|(_, value)| value == "eu")),
        "{:?}",
        status.label
    );
    assert!(status.label.iter().any(|part| {
        part.params
            .iter()
            .any(|(_, v)| v == "someone@example.invalid")
    }));
    // Space left to upload into is not remaining download traffic, and is not reported as it.
    assert_eq!(status.traffic_left, None);
    // The probe started at the host pCloud documents without qualification and corrected once.
    assert_eq!(
        host.requests()
            .iter()
            .map(|s| s.host.clone())
            .collect::<Vec<_>>(),
        vec!["api.pcloud.com".to_owned(), "eapi.pcloud.com".to_owned()]
    );
}

/// A batch check answers per link, and a link pCloud will not open is offline rather than
/// unknown.
#[tokio::test]
async fn a_check_tells_a_present_file_from_one_pcloud_will_not_open() {
    let present = MockPCloud::in_region(Region::Us);
    let checks = PCloudResolver::new(present as Arc<dyn ResolverHost>)
        .check(CheckRequest {
            urls: vec![
                OWN_FILE_US.parse().expect("URL"),
                "https://ddownload.com/f/abc".parse().expect("URL"),
            ],
            client: client(AccountId::new()),
        })
        .await
        .expect("checks");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].file_name.as_deref(), Some("release.bin"));
    assert!(matches!(
        checks[0].status,
        rd_plugin_types::LinkStatus::Online
    ));
    assert!(matches!(
        checks[1].status,
        rd_plugin_types::LinkStatus::Unknown
    ));

    let gone = PCloudResolver::new(MockPCloud::refusing(2009) as Arc<dyn ResolverHost>)
        .check(CheckRequest {
            urls: vec![OWN_FILE_US.parse().expect("URL")],
            client: client(AccountId::new()),
        })
        .await
        .expect("checks");
    assert!(matches!(
        gone[0].status,
        rd_plugin_types::LinkStatus::Offline
    ));
}

/// A download host pCloud names that is not pCloud's own never becomes an address.
#[tokio::test]
async fn a_download_host_that_is_not_pclouds_is_refused() {
    struct Foreign(Arc<MockPCloud>);
    #[async_trait]
    impl ResolverHost for Foreign {
        async fn http_request(
            &self,
            client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            let is_link = request.url.path().ends_with("getfilelink");
            let mut response = self.0.http_request(client, request).await?;
            if is_link {
                response.body =
                    br#"{"result":0,"path":"/x/release.bin","hosts":["evil.test"]}"#.to_vec();
            }
            Ok(response)
        }

        async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool {
            self.0.secret_available(account_id, reference).await
        }
    }
    let failure = PCloudResolver::new(Arc::new(Foreign(MockPCloud::in_region(Region::Us))))
        .resolve(ResolveRequest {
            url: OWN_FILE_US.parse().expect("URL"),
            client: client(AccountId::new()),
        })
        .await
        .expect_err("a foreign host is not an address");
    assert_eq!(
        failure.code.as_deref(),
        Some("pcloud.invalid_download_host")
    );
}
