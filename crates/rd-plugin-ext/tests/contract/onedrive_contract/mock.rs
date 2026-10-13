//! The mock Microsoft Graph the OneDrive contract module drives the crawler against: a share's
//! root and its items, `/children` pages joined by `@odata.nextLink`, and a root that may turn
//! out to be one file.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::extension::FolderCrawler;

use crate::support::storage::{crawler::build, table::ONEDRIVE};

const ITEM_NOT_FOUND: &str = include_str!("../../fixtures/onedrive/item_not_found.json");

/// One `/children` answer, as one page of one folder.
pub type Pages = Vec<String>;

/// What the shared root is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Folder,
    File,
}

/// The mock Graph API.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:onedrive_access_token}}` and never as a value.
pub struct MockGraph {
    root: Root,
    /// `(item id — empty for the shared root — its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockGraph {
    pub fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            root: Root::Folder,
            folders,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    pub fn single_file() -> Arc<Self> {
        Arc::new(Self {
            root: Root::File,
            folders: Vec::new(),
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

/// One `/children` page. `next` becomes the `@odata.nextLink` Graph would send: the same
/// address with a `$skiptoken`, which is how the mock tells the pages apart.
pub fn page(entries: &[String], next: Option<&str>) -> String {
    let link = next.map_or(String::new(), |token| {
        format!(
            r#""@odata.nextLink":"https://graph.microsoft.com/v1.0/shares/x/driveItem/children?$skiptoken={token}","#
        )
    });
    format!(r#"{{{link}"value":[{}]}}"#, entries.join(","))
}

/// A page whose next link points away from Graph.
pub fn page_leaving_graph(entries: &[String]) -> String {
    format!(
        r#"{{"@odata.nextLink":"https://evil.test/children?$skiptoken=x","value":[{}]}}"#,
        entries.join(",")
    )
}

pub fn file(id: &str, name: &str, size: u64) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","size":{size},"file":{{"mimeType":"video/x-matroska"}}}}"#
    )
}

pub fn subfolder(id: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","name":"{name}","folder":{{"childCount":1}}}}"#)
}

#[async_trait]
impl ResolverHost for MockGraph {
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
        // `/shares/{s}/driveItem[/children]` for the root, `/shares/{s}/items/{id}[/children]`
        // below it. Told apart by the route, the way Graph tells them apart, rather than by the
        // order the plugin happens to make them in.
        let segments: Vec<&str> = request
            .url
            .path_segments()
            .map(|segments| segments.collect())
            .unwrap_or_default();
        let (folder, listing) = match segments.as_slice() {
            ["v1.0", "shares", _, "driveItem"] => ("", false),
            ["v1.0", "shares", _, "driveItem", "children"] => ("", true),
            ["v1.0", "shares", _, "items", id] => (*id, false),
            ["v1.0", "shares", _, "items", id, "children"] => (*id, true),
            _ => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("unexpected {}", request.url.path()));
                return answer(404, ITEM_NOT_FOUND.to_owned());
            }
        };
        // The next-page token rides in the URL, because the plugin follows Graph's own
        // `@odata.nextLink` as given; the first page's `$select` and `$top` ride as templates.
        let token = request
            .url
            .query_pairs()
            .find(|(name, _)| name == "$skiptoken")
            .map(|(_, value)| value.into_owned());
        if listing {
            self.requests.lock().expect("requests").push(format!(
                "list {folder} {}",
                token.clone().unwrap_or_default()
            ));
            let index = token
                .and_then(|token| token.strip_prefix("page-").map(str::to_owned))
                .and_then(|number| number.parse::<usize>().ok())
                .unwrap_or(0);
            let pages = self
                .folders
                .iter()
                .find(|(id, _, _)| *id == folder)
                .map(|(_, _, pages)| pages);
            return match pages.and_then(|pages| pages.get(index)) {
                Some(body) => answer(200, body.clone()),
                None => answer(200, page(&[], None)),
            };
        }
        self.requests
            .lock()
            .expect("requests")
            .push(format!("get {folder}"));
        if folder.is_empty() && self.root == Root::File {
            return answer(
                200,
                r#"{"id":"redacted-item-id-0001","name":"release.bin","size":1048576,"file":{}}"#
                    .to_owned(),
            );
        }
        match self.folders.iter().find(|(known, _, _)| *known == folder) {
            Some((id, name, _)) => answer(
                200,
                format!(
                    r#"{{"id":"{}","name":"{name}","folder":{{"childCount":1}}}}"#,
                    if id.is_empty() { "root-id" } else { id }
                ),
            ),
            None => answer(404, ITEM_NOT_FOUND.to_owned()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

pub fn crawler(host: Arc<MockGraph>, bytes: &[u8]) -> FolderCrawler {
    build(&ONEDRIVE.crawler, bytes, host)
}
