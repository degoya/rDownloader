//! The mock pCloud API, in two installations: exactly one holds the account, the files and the
//! link codes, and the other answers the way pCloud does, 2094 to a token and 7001 to a link
//! code, both under HTTP 200.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::extension::FolderCrawler;

use super::US_API;
use crate::support::storage::{crawler::build, table::PCLOUD};

const USER_INFO: &str = include_str!("../../fixtures/pcloud/userinfo.json");
const LISTFOLDER_ROOT: &str = include_str!("../../fixtures/pcloud/listfolder_root.json");
const LISTFOLDER_SEASON: &str = include_str!("../../fixtures/pcloud/listfolder_season.json");
const LISTFOLDER_EMPTY: &str = include_str!("../../fixtures/pcloud/listfolder_empty.json");
const SHOWPUBLINK_FOLDER: &str = include_str!("../../fixtures/pcloud/showpublink_folder.json");
pub const SHOWPUBLINK_FILE: &str = include_str!("../../fixtures/pcloud/showpublink_file.json");
const ERROR_NOT_FOUND: &str = include_str!("../../fixtures/pcloud/error_file_not_found.json");
const ERROR_INVALID_TOKEN: &str =
    include_str!("../../fixtures/pcloud/error_invalid_access_token.json");
const ERROR_LINK: &str = include_str!("../../fixtures/pcloud/error_link_unavailable.json");

/// One recorded request: the installation it went to, the method, and its parameters.
#[derive(Clone, Debug)]
pub struct Sent {
    host: String,
    method: String,
    parameters: Vec<(String, String)>,
}

impl Sent {
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The mock pCloud API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:pcloud_access_token}}` and never as a value.
///
/// Exactly one of pCloud's two installations holds anything. The other answers the way pCloud
/// does: 2094 to a token, 7001 to a link code, both under HTTP 200.
pub struct MockPCloud {
    home: &'static str,
    /// What `showpublink` answers with — a folder's tree, or one file.
    publink: &'static str,
    /// Whether the crawled folder holds anything.
    empty: bool,
    requests: Mutex<Vec<Sent>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockPCloud {
    pub fn new(home: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            publink: SHOWPUBLINK_FOLDER,
            empty: false,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn with_publink(home: &'static str, publink: &'static str) -> Arc<Self> {
        Arc::new(Self {
            home,
            publink,
            empty: false,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn empty_folder() -> Arc<Self> {
        Arc::new(Self {
            home: US_API,
            publink: SHOWPUBLINK_FOLDER,
            empty: true,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn requests(&self) -> Vec<Sent> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }

    pub fn route(&self) -> Vec<(String, String)> {
        self.requests()
            .into_iter()
            .map(|sent| (sent.host, sent.method))
            .collect()
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
                // pCloud answers HTTP 200 to its own refusals too, which is the whole point.
                status: 200,
                final_url: request.url.clone(),
                headers: vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
                body: body.as_bytes().to_vec(),
            })
        };
        // The other installation knows nothing of this account.
        if host != self.home {
            return answer(if method.contains("publink") {
                ERROR_LINK
            } else {
                ERROR_INVALID_TOKEN
            });
        }
        match method.as_str() {
            "userinfo" => answer(USER_INFO),
            "showpublink" => answer(self.publink),
            "listfolder" => match parameters
                .iter()
                .find(|(name, _)| name == "folderid")
                .map(|(_, value)| value.as_str())
            {
                _ if self.empty => answer(LISTFOLDER_EMPTY),
                Some("42") => answer(LISTFOLDER_ROOT),
                Some("43") => answer(LISTFOLDER_SEASON),
                _ => answer(ERROR_NOT_FOUND),
            },
            _ => answer(ERROR_NOT_FOUND),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

pub fn crawler(host: Arc<MockPCloud>, bytes: &[u8]) -> FolderCrawler {
    build(&PCLOUD.crawler, bytes, host)
}
