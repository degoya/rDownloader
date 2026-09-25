//! The mock MediaFire API and the captured documents the contract test drives the component
//! with. Nothing here opens a socket: every answer is a fixture or generated from one.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_host::{PluginManifest, extension::FolderCrawler};

pub const MANIFEST: &str = include_str!("../../../../plugins/mediafire-crawler/manifest.toml");

pub const FOLDER_INFO: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-folder-get-info-2026-09-21.json"
);
pub const CONTENT_FILES: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-folder-get-content-files-2026-09-21.json"
);
pub const CONTENT_FOLDERS: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-folder-get-content-folders-2026-09-21.json"
);
pub const MISSING_TOKEN: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-folder-get-info-missing-token-2026-09-21.json"
);
pub const FILE_INFO_FOLDERKEY: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-file-get-info-folderkey-2026-09-21.json"
);
pub const FILE_INFO_BATCH: &str = include_str!(
    "../../../../plugins/mediafire/tests/fixtures/api-file-get-info-batch-2026-09-21.json"
);
pub const RATE_LIMITED: &str =
    include_str!("../../../../plugins/mediafire-crawler/tests/fixtures/api-error-261.json");
pub const PRIVATE_FOLDER: &str = include_str!(
    "../../../../plugins/mediafire-crawler/tests/fixtures/api-folder-get-info-private.json"
);

/// The captured public folder.
pub const ROOT: &str = "rww7bhhi0yc1l";
pub const ROOT_NAME: &str = "Droidfeats Galaxy S9 Walls";
/// Its two subfolders, as the captured `content_type=folders` chunk names them.
pub const EMPTY_FOLDER: &str = "x67lis2zygml5";
pub const TEST_FOLDER: &str = "gtrp6u25m6nmb";
/// A subfolder of `TestFolder`, generated.
pub const SUB_FOLDER: &str = "abcdefghijklm";

/// A bundled component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
pub fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-mediafire-crawler")
}

pub fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the crawler manifest")
}

/// A generated `folder/get_info` answer in the captured shape.
pub fn info(key: &str, name: &str) -> String {
    format!(
        r#"{{"response":{{"action":"folder/get_info","folder_info":{{"folderkey":"{key}","name":"{name}","privacy":"public","file_count":"0","folder_count":"0"}},"result":"Success","current_api_version":"1.5"}}}}"#
    )
}

/// A generated `folder/get_content` chunk in the captured shape.
pub fn chunk(key: &str, content_type: &str, number: u32, entries: &[String], more: bool) -> String {
    let list = if content_type == "files" {
        "files"
    } else {
        "folders"
    };
    let more = if more { "yes" } else { "no" };
    format!(
        r#"{{"response":{{"action":"folder/get_content","folder_content":{{"chunk_size":"100","content_type":"{content_type}","chunk_number":"{number}","folderkey":"{key}","{list}":[{}],"more_chunks":"{more}"}},"result":"Success","current_api_version":"1.5"}}}}"#,
        entries.join(",")
    )
}

pub fn file(key: &str, name: &str, size: u64) -> String {
    format!(
        r#"{{"quickkey":"{key}","filename":"{name}","size":"{size}","privacy":"public","password_protected":"no","hash":"0000000000000000000000000000000000000000000000000000000000000000"}}"#
    )
}

pub fn locked_file(key: &str, name: &str) -> String {
    format!(
        r#"{{"quickkey":"{key}","filename":"{name}","size":"1","privacy":"public","password_protected":"yes"}}"#
    )
}

pub fn subfolder(key: &str, name: &str) -> String {
    format!(r#"{{"folderkey":"{key}","name":"{name}","privacy":"public"}}"#)
}

/// One folder as the mock holds it: its `get_info` answer and its chunks per content type.
pub struct Folder {
    pub key: &'static str,
    pub info: String,
    /// `(content_type, chunk number) -> document`.
    pub chunks: Vec<((&'static str, u32), String)>,
}

/// The mock MediaFire API: `folder/get_info`, `folder/get_content` and `file/get_info`,
/// answered from documents keyed by the query the plugin sends.
pub struct MockMediafire {
    folders: Vec<Folder>,
    /// The `file/get_info` answer, for a key list.
    files: Option<String>,
    requests: Mutex<Vec<String>>,
    /// When set, every request is answered with this status and body instead.
    failure: Option<(u16, &'static str)>,
}

impl MockMediafire {
    pub fn new(folders: Vec<Folder>) -> Arc<Self> {
        Arc::new(Self {
            folders,
            files: None,
            requests: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    pub fn with_files(document: &str) -> Arc<Self> {
        Arc::new(Self {
            folders: Vec::new(),
            files: Some(document.to_owned()),
            requests: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    pub fn failing(status: u16, body: &'static str) -> Arc<Self> {
        Arc::new(Self {
            folders: Vec::new(),
            files: None,
            requests: Mutex::new(Vec::new()),
            failure: Some((status, body)),
        })
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

/// The captured folder with its two captured subfolders, the deeper levels generated.
pub fn captured_tree() -> Vec<Folder> {
    vec![
        Folder {
            key: ROOT,
            info: FOLDER_INFO.to_owned(),
            chunks: vec![
                (("folders", 1), CONTENT_FOLDERS.to_owned()),
                (("files", 1), CONTENT_FILES.to_owned()),
            ],
        },
        Folder {
            key: EMPTY_FOLDER,
            info: info(EMPTY_FOLDER, "EmptyFolder"),
            chunks: vec![
                (
                    ("folders", 1),
                    chunk(EMPTY_FOLDER, "folders", 1, &[], false),
                ),
                (("files", 1), chunk(EMPTY_FOLDER, "files", 1, &[], false)),
            ],
        },
        Folder {
            key: TEST_FOLDER,
            info: info(TEST_FOLDER, "TestFolder"),
            chunks: vec![
                (
                    ("folders", 1),
                    chunk(
                        TEST_FOLDER,
                        "folders",
                        1,
                        &[subfolder(SUB_FOLDER, "Sub")],
                        false,
                    ),
                ),
                (
                    ("files", 1),
                    chunk(
                        TEST_FOLDER,
                        "files",
                        1,
                        &[
                            file("5jd1tk6le587um7", "readme.txt", 12),
                            locked_file("5jd1tk6le587um8", "secret.zip"),
                            file("5jd1tk6le587um9", "notes.txt", 34),
                        ],
                        false,
                    ),
                ),
            ],
        },
        Folder {
            key: SUB_FOLDER,
            info: info(SUB_FOLDER, "Sub"),
            chunks: vec![
                (("folders", 1), chunk(SUB_FOLDER, "folders", 1, &[], false)),
                (
                    ("files", 1),
                    chunk(
                        SUB_FOLDER,
                        "files",
                        1,
                        &[file("5jd1tk6le587uma", "deep.bin", 56)],
                        false,
                    ),
                ),
            ],
        },
    ]
}

pub fn answer(
    status: u16,
    body: String,
    request: &HostHttpRequest,
) -> Result<HostHttpResponse, Failure> {
    Ok(HostHttpResponse {
        status,
        final_url: request.url.clone(),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body: body.into_bytes(),
    })
}

#[async_trait]
impl ResolverHost for MockMediafire {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let query = |name: &str| -> String {
            request
                .query
                .iter()
                .find(|item| item.name == name)
                .map(|item| item.value_template.clone())
                .unwrap_or_default()
        };
        let call = request.url.path().to_owned();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url.host_str(), Some("www.mediafire.com"));
        assert_eq!(query("response_format"), "json");
        let record = format!(
            "{call}?folder_key={}&content_type={}&chunk={}&quick_key={}",
            query("folder_key"),
            query("content_type"),
            query("chunk"),
            query("quick_key")
        );
        self.requests.lock().expect("requests").push(record);
        if let Some((status, body)) = self.failure {
            return answer(status, body.to_owned(), &request);
        }
        match call.as_str() {
            "/api/1.5/folder/get_info.php" => {
                let key = query("folder_key");
                match self.folders.iter().find(|folder| folder.key == key) {
                    Some(folder) => answer(200, folder.info.clone(), &request),
                    None => answer(400, MISSING_TOKEN.to_owned(), &request),
                }
            }
            "/api/1.5/folder/get_content.php" => {
                let key = query("folder_key");
                let wanted = (
                    query("content_type"),
                    query("chunk").parse::<u32>().unwrap_or(1),
                );
                let found =
                    self.folders
                        .iter()
                        .find(|folder| folder.key == key)
                        .and_then(|folder| {
                            folder.chunks.iter().find(|((kind, number), _)| {
                                *kind == wanted.0 && *number == wanted.1
                            })
                        });
                match found {
                    Some((_, document)) => answer(200, document.clone(), &request),
                    None => answer(400, MISSING_TOKEN.to_owned(), &request),
                }
            }
            "/api/1.5/file/get_info.php" => match &self.files {
                Some(document) => answer(200, document.clone(), &request),
                None => answer(400, FILE_INFO_FOLDERKEY.to_owned(), &request),
            },
            _ => answer(404, String::new(), &request),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

pub fn crawler(host: Arc<MockMediafire>, bytes: &[u8]) -> FolderCrawler {
    FolderCrawler::new(manifest(), bytes, Some(host)).expect("compile the crawler")
}
