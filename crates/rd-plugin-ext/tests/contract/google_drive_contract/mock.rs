//! The mock Drive API the Google Drive contract module drives the crawler against: folders by
//! id, `files.list` pages joined by their `nextPageToken`, and `files.get` for the folder itself.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::extension::FolderCrawler;

use crate::support::storage::{crawler::build, table::GOOGLE_DRIVE};

/// One `files.list` answer, as one page of one folder.
pub type Pages = Vec<String>;

/// The mock Drive API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:google_drive_access_token}}` and never as a value.
pub struct MockDrive {
    /// `(folder id, its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockDrive {
    pub fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }
}

/// One `files.list` page.
pub fn page(entries: &[String], next: Option<&str>) -> String {
    let token = next.map_or(String::new(), |token| {
        format!(r#""nextPageToken":"{token}","#)
    });
    format!(r#"{{{token}"files":[{}]}}"#, entries.join(","))
}

pub fn file(id: &str, name: &str, size: u64) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","mimeType":"video/x-matroska","size":"{size}"}}"#)
}

pub fn document(id: &str, name: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","mimeType":"application/vnd.google-apps.spreadsheet"}}"#
    )
}

pub fn subfolder(id: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","mimeType":"application/vnd.google-apps.folder"}}"#)
}

#[async_trait]
impl ResolverHost for MockDrive {
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
        let value = |name: &str| {
            request
                .query
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.value_template.clone())
        };
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
        // `files.list` carries `q`; `files.get` does not. Told apart the way Drive tells them
        // apart, by the request rather than by the order the plugin happens to make them in.
        match value("q") {
            Some(query) => {
                let parent = query.split('\'').nth(1).unwrap_or_default().to_owned();
                let token = value("pageToken");
                self.requests.lock().expect("requests").push(format!(
                    "list {parent} {}",
                    token.clone().unwrap_or_default()
                ));
                let index = token
                    .and_then(|token| token.strip_prefix("page-").map(str::to_owned))
                    .and_then(|number| number.parse::<usize>().ok())
                    .unwrap_or(0);
                let pages = self
                    .folders
                    .iter()
                    .find(|(id, _, _)| *id == parent)
                    .map(|(_, _, pages)| pages);
                match pages.and_then(|pages| pages.get(index)) {
                    Some(body) => answer(200, body.clone()),
                    None => answer(200, page(&[], None)),
                }
            }
            None => {
                let id = request
                    .url
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("get {id}"));
                match self.folders.iter().find(|(known, _, _)| *known == id) {
                    Some((id, name, _)) => answer(
                        200,
                        format!(
                            r#"{{"id":"{id}","name":"{name}",
                                 "mimeType":"application/vnd.google-apps.folder"}}"#
                        ),
                    ),
                    None => answer(
                        404,
                        r#"{"error":{"code":404,"errors":[{"reason":"notFound"}]}}"#.to_owned(),
                    ),
                }
            }
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

pub fn crawler(host: Arc<MockDrive>, bytes: &[u8]) -> FolderCrawler {
    build(&GOOGLE_DRIVE.crawler, bytes, host)
}
