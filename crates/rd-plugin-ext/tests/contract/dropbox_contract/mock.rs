//! The mock Dropbox API the Dropbox contract module drives the crawler against: folders by path,
//! `list_folder` pages joined by their cursors, and a shared link that may want a password.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::extension::FolderCrawler;

use crate::support::storage::{crawler::build, table::DROPBOX};

const ERROR_NOT_FOUND: &str = include_str!("../../fixtures/dropbox/error_path_not_found.json");
const ERROR_ACCESS_DENIED: &str =
    include_str!("../../fixtures/dropbox/error_shared_link_access_denied.json");

/// One folder the mock holds: its API path (absolute in the account, relative to the link's
/// root inside a shared link), its name, and the `list_folder` pages it answers with, in order.
pub struct Folder {
    pub path: &'static str,
    pub name: &'static str,
    pub pages: Vec<String>,
}

/// The mock Dropbox API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:dropbox_access_token}}` and never as a value.
/// Pages are followed the way Dropbox follows them: `list_folder/continue` is answered with the
/// page after the one whose `cursor` was quoted, so a walk that forgot the cursor repeats page
/// one and a walk that invented one gets nothing.
pub struct MockDropbox {
    folders: Vec<Folder>,
    /// The password the shared link wants, when it wants one.
    link_password: Option<&'static str>,
    requests: Mutex<Vec<String>>,
    bodies: Mutex<Vec<serde_json::Value>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockDropbox {
    pub fn new(folders: Vec<Folder>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            link_password: None,
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn password_protected(folders: Vec<Folder>, password: &'static str) -> Arc<Self> {
        Arc::new(Self {
            folders,
            link_password: Some(password),
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn bodies(&self) -> Vec<serde_json::Value> {
        self.bodies.lock().expect("bodies").clone()
    }

    pub fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }

    fn folder(&self, path: &str) -> Option<&Folder> {
        self.folders
            .iter()
            .find(|folder| folder.path.eq_ignore_ascii_case(path))
    }

    /// The page after the one that carried `cursor`, wherever it was.
    fn page_after(&self, cursor: &str) -> Option<&String> {
        let quoted = format!("\"cursor\": \"{cursor}\"");
        let compact = format!("\"cursor\":\"{cursor}\"");
        self.folders.iter().find_map(|folder| {
            let index = folder
                .pages
                .iter()
                .position(|page| page.contains(&quoted) || page.contains(&compact))?;
            folder.pages.get(index + 1)
        })
    }
}

/// One `list_folder` page, generated: `entries`, a cursor naming the folder and the page, and
/// whether there is more.
pub fn page(folder: &str, index: usize, entries: &[String], more: bool) -> String {
    format!(
        r#"{{"entries":[{}],"cursor":"cursor:{folder}:{index}","has_more":{more}}}"#,
        entries.join(",")
    )
}

pub fn file(name: &str, size: u64) -> String {
    format!(
        r#"{{".tag":"file","name":"{name}","id":"id:{name}","rev":"015f3d2a1b2c3d4e5f6a7","size":{size},"is_downloadable":true}}"#
    )
}

pub fn paper(name: &str) -> String {
    format!(r#"{{".tag":"file","name":"{name}","id":"id:{name}","is_downloadable":false}}"#)
}

pub fn subfolder(name: &str) -> String {
    format!(r#"{{".tag":"folder","name":"{name}","id":"id:{name}"}}"#)
}

#[async_trait]
impl ResolverHost for MockDropbox {
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
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
        self.bodies.lock().expect("bodies").push(body.clone());
        let text = |name: &str| body.get(name).and_then(serde_json::Value::as_str);
        let answer = |status: u16, body: String| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
                body: body.into_bytes(),
            })
        };
        let endpoint = request.url.path().to_owned();
        // A password-protected link refuses every call that does not quote its password —
        // in `link_password` on the sharing endpoint, in `shared_link.password` on the listing.
        if let Some(expected) = self.link_password {
            let given = text("link_password").or_else(|| {
                body.get("shared_link")
                    .and_then(|link| link.get("password"))
                    .and_then(serde_json::Value::as_str)
            });
            let shared = text("url").is_some() || body.get("shared_link").is_some();
            if shared && given != Some(expected) {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("denied {endpoint}"));
                return answer(409, ERROR_ACCESS_DENIED.to_owned());
            }
        }
        match endpoint.as_str() {
            "/2/files/get_metadata" | "/2/sharing/get_shared_link_metadata" => {
                let path = text("path").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("metadata {path}"));
                match self.folder(&path) {
                    Some(folder) => answer(
                        200,
                        format!(
                            r#"{{".tag":"folder","name":"{}","id":"id:folder"}}"#,
                            folder.name
                        ),
                    ),
                    None => answer(409, ERROR_NOT_FOUND.to_owned()),
                }
            }
            "/2/files/list_folder" => {
                let path = text("path").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("list {path}"));
                match self.folder(&path).and_then(|folder| folder.pages.first()) {
                    Some(first) => answer(200, first.clone()),
                    None => answer(409, ERROR_NOT_FOUND.to_owned()),
                }
            }
            "/2/files/list_folder/continue" => {
                let cursor = text("cursor").unwrap_or_default().to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("continue {cursor}"));
                match self.page_after(&cursor) {
                    Some(next) => answer(200, next.clone()),
                    None => answer(
                        409,
                        r#"{"error_summary":"reset/..","error":{".tag":"reset"}}"#.to_owned(),
                    ),
                }
            }
            _ => answer(404, String::new()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

pub fn crawler(host: Arc<MockDropbox>, bytes: &[u8]) -> FolderCrawler {
    build(&DROPBOX.crawler, bytes, host)
}
