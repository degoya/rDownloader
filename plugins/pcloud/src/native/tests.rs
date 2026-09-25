//! Native-adapter coverage for pCloud, driven against a mock pCloud API that has two
//! installations.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! neither `api.pcloud.com` nor `eapi.pcloud.com` is contacted. A run against a real pCloud
//! account is *not* claimed here; there is none in this checkout, and there is no registered
//! application either. What is proven is everything that does not need one — and above all the
//! thing pCloud is easiest to get subtly wrong: the mock holds the account in **one** of the
//! two installations and answers `result: 2094` at the other, exactly as pCloud does, so a
//! plugin that guessed the region and gave up would fail these tests rather than look like a
//! bad credential in production.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pcloud_common::address::Region;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CheckRequest, ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, Resolver,
    ResolverHost,
};

use super::PCloudResolver;

/// The same file, spelled in each installation's own addresses.
const OWN_FILE_US: &str = "https://my.pcloud.com/#/filemanager?folder=42&fileid=123";
const OWN_FILE_EU: &str = "https://e.pcloud.com/#/filemanager?folder=42&fileid=123";
const PUBLIC_FILE_EU: &str = "https://e.pcloud.link/publink/show?code=XZredacted&fileid=7";

const STAT: &str = r#"{"result":0,"metadata":{"name":"release.bin","isfolder":false,
    "fileid":123,"parentfolderid":42,"size":1048576,"contenttype":"application/octet-stream",
    "modified":"Fri, 02 Jan 2026 03:04:06 +0000","hash":9876543210}}"#;
const STAT_FOLDER: &str = r#"{"result":0,"metadata":{"name":"Show","isfolder":true,
    "folderid":123,"parentfolderid":0}}"#;
const CHECKSUM: &str = r#"{"result":0,
    "sha1":"0000000000000000000000000000000000000001",
    "sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "metadata":{"name":"release.bin","isfolder":false,"fileid":123,"size":1048576}}"#;
const FILE_LINK: &str = r#"{"result":0,"expires":"Fri, 02 Jan 2026 09:04:06 +0000",
    "path":"/cBZredacted/release.bin","hosts":["edef2.pcloud.com","evd4.pcloud.com"]}"#;
const PUBLINK: &str = r#"{"result":0,"metadata":{"name":"Shared","isfolder":true,
    "folderid":9,"contents":[
      {"name":"release.bin","isfolder":false,"fileid":7,"size":2048,"parentfolderid":9}]}}"#;
/// A public link that *is* one file. pCloud states a `parentfolderid` on it all the same — the
/// owner's folder — so the difference between this and the tree above cannot be read off that
/// field, which is why `Described::link_is_folder` is carried rather than re-derived.
const PUBLINK_FILE: &str = r#"{"result":0,"metadata":{"name":"release.bin","isfolder":false,
    "fileid":7,"size":2048,"parentfolderid":9,"contenttype":"application/octet-stream"}}"#;
const USER_INFO: &str = r#"{"result":0,"email":"someone@example.invalid",
    "emailverified":true,"premium":true,"quota":2199023255552,"usedquota":1024}"#;

/// One recorded request: the installation it went to, the method, and the parameters.
#[derive(Clone, Debug)]
struct Sent {
    host: String,
    method: String,
    parameters: Vec<(String, String)>,
}

impl Sent {
    fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A mock pCloud with two installations, exactly one of which holds the account.
struct MockPCloud {
    /// Where the account and its data actually live. The other installation answers 2094.
    home: Region,
    /// What `showpublink` answers with — a folder's tree, or the file itself.
    publink: &'static str,
    /// When set, every method answers this `result` instead of its own answer.
    refusal: Option<u64>,
    requests: Mutex<Vec<Sent>>,
    authorizations: Mutex<Vec<String>>,
    has_token: bool,
}

impl MockPCloud {
    fn in_region(home: Region) -> Arc<Self> {
        Self::with_publink(home, PUBLINK)
    }

    fn with_publink(home: Region, publink: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            publink,
            refusal: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn refusing(result: u64) -> Arc<Self> {
        Arc::new(Self {
            home: Region::Us,
            publink: PUBLINK,
            refusal: Some(result),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: true,
        })
    }

    fn signed_out() -> Arc<Self> {
        Arc::new(Self {
            home: Region::Us,
            publink: PUBLINK,
            refusal: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            has_token: false,
        })
    }

    fn requests(&self) -> Vec<Sent> {
        self.requests.lock().expect("requests").clone()
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }
}

#[async_trait]
impl ResolverHost for MockPCloud {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
        }
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let method = request.url.path().trim_start_matches('/').to_owned();
        let parameters: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        self.requests.lock().expect("requests").push(Sent {
            host: host.clone(),
            method: method.clone(),
            parameters: parameters.clone(),
        });
        let answer = |body: &str| {
            Ok(HostHttpResponse {
                // pCloud answers 200 to its own refusals too, which is the whole point.
                status: 200,
                final_url: request.url.clone(),
                headers: vec![rd_plugin_api::ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
                body: body.as_bytes().to_vec(),
            })
        };
        if let Some(result) = self.refusal {
            return answer(&format!(
                r#"{{"result":{result},"error":"A sentence pCloud wrote."}}"#
            ));
        }
        // The other installation knows nothing of this account, and says so the way pCloud
        // does: `2094` for a token it will not accept, `7001` for a link code it never issued.
        if host != self.home.api_host() {
            let result = if method.contains("publink") {
                7001
            } else {
                2094
            };
            return answer(&format!(
                r#"{{"result":{result},"error":"A sentence pCloud wrote."}}"#
            ));
        }
        answer(match method.as_str() {
            "userinfo" => USER_INFO,
            "stat" => STAT,
            "checksumfile" => CHECKSUM,
            "getfilelink" | "getpublinkdownload" => FILE_LINK,
            "showpublink" => self.publink,
            _ => r#"{"result":1004,"error":"A sentence pCloud wrote."}"#,
        })
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        self.has_token && reference == "pcloud_access_token"
    }
}

fn client(account: AccountId) -> ClientIdentity {
    ClientIdentity {
        account_id: Some(account),
        proxy_profile_id: None,
        tls_revision: 7,
    }
}

async fn resolve(
    host: Arc<MockPCloud>,
    url: &str,
) -> Result<rd_plugin_api::ResolvedDownload, Failure> {
    let account = AccountId::new();
    PCloudResolver::new(host)
        .resolve(ResolveRequest {
            url: url.parse().expect("URL"),
            client: client(account),
        })
        .await
}

/// One file in the account's own drive, through the official methods and nothing else.
#[tokio::test]
async fn an_own_file_is_resolved_through_stat_checksumfile_and_getfilelink() {
    let host = MockPCloud::in_region(Region::Us);
    let resolved = resolve(Arc::clone(&host), OWN_FILE_US)
        .await
        .expect("a download");

    assert_eq!(
        resolved.url.as_str(),
        "https://edef2.pcloud.com/cBZredacted/release.bin"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(1_048_576));
    // Europe answers `sha256`, the United States `md5`; both answer `sha1`. The strongest on
    // offer is what travels to the verifier.
    let checksum = resolved.checksum.expect("a checksum");
    assert_eq!(checksum.algorithm, rd_core::ChecksumAlgorithm::Sha256);
    assert_eq!(
        checksum.value,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    // Nothing is repeated on the transfer: the ticket pCloud handed back is already
    // authorised, and the account's token has no business at a content server.
    assert!(resolved.headers.is_empty());

    let sent = host.requests();
    assert_eq!(
        sent.iter().map(|s| s.method.as_str()).collect::<Vec<_>>(),
        vec!["stat", "checksumfile", "getfilelink"]
    );
    assert!(sent.iter().all(|s| s.host == "api.pcloud.com"));
    assert_eq!(sent[2].parameter("forcedownload"), Some("1"));
    assert_eq!(sent[0].parameter("fileid"), Some("123"));
}

/// **The region.** The address says one installation, the account lives in the other, and
/// pCloud answers the first one exactly as it answers a bad token. One correction, at the
/// other installation, and then every further call of the same resolve goes straight there.
#[tokio::test]
async fn an_address_naming_the_wrong_installation_is_corrected_once_and_then_pinned() {
    let host = MockPCloud::in_region(Region::Eu);
    let resolved = resolve(Arc::clone(&host), OWN_FILE_US)
        .await
        .expect("a download");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));

    let sent = host.requests();
    assert_eq!(
        sent.iter()
            .map(|s| (s.host.as_str(), s.method.as_str()))
            .collect::<Vec<_>>(),
        vec![
            // The address said the United States, so that is where it started.
            ("api.pcloud.com", "stat"),
            // 2094 there, corrected once.
            ("eapi.pcloud.com", "stat"),
            // And pinned: the two calls after it pay nothing.
            ("eapi.pcloud.com", "checksumfile"),
            ("eapi.pcloud.com", "getfilelink"),
        ],
        "the correction must happen once and then stop happening"
    );
}

/// An address that names the right installation pays nothing at all.
#[tokio::test]
async fn an_address_naming_the_right_installation_never_asks_the_other() {
    let host = MockPCloud::in_region(Region::Eu);
    resolve(Arc::clone(&host), OWN_FILE_EU)
        .await
        .expect("a download");
    assert!(
        host.requests()
            .iter()
            .all(|sent| sent.host == "eapi.pcloud.com"),
        "{:?}",
        host.requests()
    );
}

/// A public link is opened without the account's token going anywhere near it, and the file
/// inside it comes out of the tree `showpublink` already answered with.
#[tokio::test]
async fn a_public_link_is_opened_without_a_credential() {
    let host = MockPCloud::in_region(Region::Eu);
    let resolved = resolve(Arc::clone(&host), PUBLIC_FILE_EU)
        .await
        .expect("a download");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));
    assert_eq!(resolved.size.map(|size| size.get()), Some(2048));
    // pCloud states no checksum for a public link, and nothing is invented to fill the gap.
    assert!(resolved.checksum.is_none());

    let sent = host.requests();
    assert_eq!(
        sent.iter().map(|s| s.method.as_str()).collect::<Vec<_>>(),
        vec!["showpublink", "getpublinkdownload"]
    );
    assert_eq!(sent[1].parameter("code"), Some("XZredacted"));
    // The link points at a folder, so `fileid` says which file inside it.
    assert_eq!(sent[1].parameter("fileid"), Some("7"));
    assert!(
        host.authorizations().is_empty(),
        "a public link needs no credential, so none may be sent: {:?}",
        host.authorizations()
    );
}

/// A public link that *is* one file is asked for without a `fileid`, which is what pCloud
/// documents: the parameter is required only once the link points at a folder. It cannot be
/// decided from the metadata — pCloud states a `parentfolderid` either way — so it is decided
/// from the shape of the answer.
#[tokio::test]
async fn a_public_link_that_is_one_file_is_asked_for_without_a_fileid() {
    let host = MockPCloud::with_publink(Region::Eu, PUBLINK_FILE);
    let resolved = resolve(Arc::clone(&host), PUBLIC_FILE_EU)
        .await
        .expect("a download");
    assert_eq!(resolved.file_name.as_deref(), Some("release.bin"));

    let sent = host.requests();
    assert_eq!(
        sent.iter().map(|s| s.method.as_str()).collect::<Vec<_>>(),
        vec!["showpublink", "getpublinkdownload"]
    );
    assert_eq!(sent[1].parameter("code"), Some("XZredacted"));
    assert_eq!(
        sent[1].parameter("fileid"),
        None,
        "the link is the file, so pCloud wants no fileid"
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_marker_and_never_as_a_value() {
    let host = MockPCloud::in_region(Region::Us);
    resolve(Arc::clone(&host), OWN_FILE_US)
        .await
        .expect("a download");
    let authorizations = host.authorizations();
    assert!(!authorizations.is_empty());
    for value in authorizations {
        assert_eq!(
            value, "Bearer {{secret:pcloud_access_token}}",
            "the plugin must name the reference, never hold the token"
        );
    }
}

/// The ways pCloud says no reach the person as different codes, none of them repeating a word
/// pCloud wrote — the number travels instead, because a number cannot carry a token.
#[tokio::test]
async fn the_refusals_pcloud_numbers_are_told_apart_and_carry_no_prose() {
    for (result, expected) in [
        (2009_u64, "pcloud.file_not_found"),
        (2003, "pcloud.download_not_permitted"),
        (4000, "pcloud.rate_limited"),
        (5000, "pcloud.unavailable"),
        (1004, "pcloud.api_refused"),
    ] {
        let failure = resolve(MockPCloud::refusing(result), OWN_FILE_US)
            .await
            .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(expected), "{result}");
        assert!(
            !format!("{failure:?}").contains("A sentence pCloud wrote"),
            "a provider's prose reached the failure: {failure:?}"
        );
        assert!(
            format!("{failure:?}").contains(&result.to_string()),
            "the number pCloud refused with has to be visible: {failure:?}"
        );
    }
    // A token neither installation accepts really is a token to sign in again with — and it
    // took both installations to say so.
    let both = MockPCloud::refusing(2094);
    let failure = resolve(Arc::clone(&both), OWN_FILE_US)
        .await
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("pcloud.sign_in_required"));
    assert_eq!(
        both.requests().len(),
        2,
        "both installations have to have refused before this is reported"
    );
    // A rate limit is not a region question and is never asked twice.
    let limited = MockPCloud::refusing(4000);
    let _ = resolve(Arc::clone(&limited), OWN_FILE_US).await;
    assert_eq!(limited.requests().len(), 1);
}

/// A folder pasted at the resolver is said plainly, and an account with no token is refused
/// before anything is asked of pCloud.
#[tokio::test]
async fn a_folder_and_a_missing_token_are_refused_by_name() {
    /// `stat` answering a folder is what a folder id pasted as a file id looks like.
    struct AlwaysFolder(Arc<MockPCloud>);
    #[async_trait]
    impl ResolverHost for AlwaysFolder {
        async fn http_request(
            &self,
            client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            let is_stat = request.url.path().ends_with("stat");
            let mut response = self.0.http_request(client, request).await?;
            if is_stat {
                response.body = STAT_FOLDER.as_bytes().to_vec();
            }
            Ok(response)
        }

        async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool {
            self.0.secret_available(account_id, reference).await
        }
    }
    let failure = PCloudResolver::new(Arc::new(AlwaysFolder(MockPCloud::in_region(Region::Us))))
        .resolve(ResolveRequest {
            url: OWN_FILE_US.parse().expect("URL"),
            client: client(AccountId::new()),
        })
        .await
        .expect_err("a folder is not a download");
    assert_eq!(failure.code.as_deref(), Some("pcloud.is_a_folder"));

    let signed_out = MockPCloud::signed_out();
    let failure = resolve(Arc::clone(&signed_out), OWN_FILE_US)
        .await
        .expect_err("no token");
    assert_eq!(failure.code.as_deref(), Some("pcloud.sign_in_required"));
    assert!(matches!(failure.category, FailureKind::AuthRequired));
    assert!(
        signed_out.requests().is_empty(),
        "an account with no token must ask pCloud nothing"
    );
}

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
    assert!(matches!(checks[0].status, rd_core::LinkStatus::Online));
    assert!(matches!(checks[1].status, rd_core::LinkStatus::Unknown));

    let gone = PCloudResolver::new(MockPCloud::refusing(2009) as Arc<dyn ResolverHost>)
        .check(CheckRequest {
            urls: vec![OWN_FILE_US.parse().expect("URL")],
            client: client(AccountId::new()),
        })
        .await
        .expect("checks");
    assert!(matches!(gone[0].status, rd_core::LinkStatus::Offline));
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
