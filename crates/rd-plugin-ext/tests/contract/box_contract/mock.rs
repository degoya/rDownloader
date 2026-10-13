//! The mock Box Content API, shared by the crawler and the resolver: folders paged by offset,
//! `shared_items` for what a shared link points at, and `files/<id>` pinned to one version.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{ComponentResolver, PluginManifest, extension::FolderCrawler};

use crate::support::storage::{crawler::build, table::BOX};

const RESOLVER_MANIFEST: &str = include_str!("../../../../../plugins/box/manifest.toml");
const ERROR_NOT_FOUND: &str = include_str!("../../fixtures/box/error_not_found.json");

/// One folder's pages: each entry is one page's rows, as Box would answer them in order.
pub type Pages = Vec<Vec<String>>;

/// What the crawled address turns out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Folder,
    File,
}

/// The mock Box API, shared by the crawler and the resolver.
///
/// It answers at the host boundary, which is also what lets a test assert that the account's
/// access token left the plugin as `{{secret:box_access_token}}` and never as a value, and that
/// a shared link's password left it only inside a `boxapi` header.
pub struct MockBox {
    root: Root,
    /// `(folder id, its display name, its pages)`.
    folders: Vec<(&'static str, &'static str, Pages)>,
    /// How many rows each folder claims to hold; `None` means "as many as it answered".
    claimed_total: Option<u64>,
    requests: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    box_apis: Mutex<Vec<Option<String>>>,
}

impl MockBox {
    pub fn new(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            root: Root::Folder,
            folders,
            claimed_total: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            box_apis: Mutex::new(Vec::new()),
        })
    }

    /// A folder that claims to hold far more rows than it will ever answer, which is how a
    /// collection paginates for as long as anybody asks.
    pub fn endless(folders: Vec<(&'static str, &'static str, Pages)>) -> Arc<Self> {
        Arc::new(Self {
            claimed_total: Some(10_000),
            ..Arc::try_unwrap(Self::new(folders)).ok().expect("one owner")
        })
    }

    pub fn single_file() -> Arc<Self> {
        Arc::new(Self {
            root: Root::File,
            ..Arc::try_unwrap(Self::new(Vec::new()))
                .ok()
                .expect("one owner")
        })
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn box_apis(&self) -> Vec<Option<String>> {
        self.box_apis.lock().expect("box apis").clone()
    }

    pub fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }
}

pub fn file_row(id: &str, name: &str, size: u64) -> String {
    format!(r#"{{"type":"file","id":"{id}","name":"{name}","size":{size},"item_status":"active"}}"#)
}

pub fn folder_row(id: &str, name: &str) -> String {
    format!(r#"{{"type":"folder","id":"{id}","name":"{name}","item_status":"active"}}"#)
}

/// The file document `/2.0/files/<id>` answers with, pinned to one version.
fn file_document(id: &str, version: &str) -> String {
    format!(
        r#"{{"type":"file","id":"{id}","name":"release.bin","size":1048576,
            "sha1":"aabbccddeeff00112233445566778899aabbccdd",
            "file_version":{{"type":"file_version","id":"{version}"}},
            "item_status":"active"}}"#
    )
}

#[async_trait]
impl ResolverHost for MockBox {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let mut box_api = None;
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
            if header.name.eq_ignore_ascii_case("boxapi") {
                box_api = Some(header.value_template.clone());
            }
        }
        self.box_apis.lock().expect("box apis").push(box_api);
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
        let offset: u64 = request
            .query
            .iter()
            .find(|value| value.name == "offset")
            .and_then(|value| value.value_template.parse().ok())
            .unwrap_or_default();
        let segments: Vec<&str> = request
            .url
            .path_segments()
            .map(Iterator::collect)
            .unwrap_or_default();
        match segments.as_slice() {
            // What a shared link points at. The one endpoint whose job is to say so.
            ["2.0", "shared_items"] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push("shared_items".to_owned());
                answer(
                    200,
                    match self.root {
                        Root::File => file_document("42", "98765"),
                        Root::Folder => {
                            let (id, name, _) = self.folders.first().expect("a root folder");
                            folder_row(id, name)
                        }
                    },
                )
            }
            ["2.0", "folders", id] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("get {id}"));
                match self.folders.iter().find(|(known, _, _)| known == id) {
                    Some((id, name, _)) => answer(200, folder_row(id, name)),
                    None => answer(404, ERROR_NOT_FOUND.to_owned()),
                }
            }
            ["2.0", "folders", id, "items"] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("list {id} {offset}"));
                let Some((_, _, pages)) = self.folders.iter().find(|(known, _, _)| known == id)
                else {
                    return answer(404, ERROR_NOT_FOUND.to_owned());
                };
                let total = self
                    .claimed_total
                    .unwrap_or_else(|| pages.iter().map(|page| page.len() as u64).sum());
                // Box paginates by offset, so the page answered is the one that starts there.
                let mut start = 0;
                for page in pages {
                    if start == offset {
                        return answer(
                            200,
                            format!(
                                r#"{{"total_count":{total},"offset":{offset},"limit":1000,"entries":[{}]}}"#,
                                page.join(",")
                            ),
                        );
                    }
                    start += page.len() as u64;
                }
                answer(
                    200,
                    format!(
                        r#"{{"total_count":{total},"offset":{offset},"limit":1000,"entries":[]}}"#
                    ),
                )
            }
            ["2.0", "files", id] => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("file {id}"));
                answer(200, file_document(id, "98765"))
            }
            _ => {
                self.requests
                    .lock()
                    .expect("requests")
                    .push(format!("unexpected {}", request.url.path()));
                answer(404, ERROR_NOT_FOUND.to_owned())
            }
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

pub fn crawler(host: Arc<MockBox>, bytes: &[u8]) -> FolderCrawler {
    build(&BOX.crawler, bytes, host)
}

pub fn resolver(host: Arc<MockBox>, bytes: &[u8]) -> ComponentResolver {
    let manifest: PluginManifest =
        toml::from_str(RESOLVER_MANIFEST).expect("the resolver manifest");
    ComponentResolver::new(manifest, bytes, host).expect("compile the resolver")
}
