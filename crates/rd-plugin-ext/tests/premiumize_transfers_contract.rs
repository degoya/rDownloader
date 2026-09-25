//! The Premiumize transfer contract, driven end to end against a mock of the provider
//! (RD-120-23).
//!
//! `plugins/premiumize-transfers/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-premiumize-transfers`
//! -- and the mock stands in for `www.premiumize.me`. It answers at the host boundary, so no
//! socket is opened, no account is needed and no request leaves the machine; and because it
//! sees each request exactly as the plugin described it, a test can assert that the account's
//! key left the plugin as the template `{{secret:premiumize_api_key}}` and never as a value.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `create.json` | a handle carrying the transfer's id |
//! | A container is submitted | `create.json` | one multipart upload naming `source.torrent` |
//! | A plain address is submitted | `create.json` | one form body carrying `src` |
//! | `queued` | `list_queued.json` | `Preparing`, with a wait |
//! | `running` | `list_running.json` | `Working`, 425 permille |
//! | `finished`, several files | `list_finished_folder.json` | `Ready`, three addresses across two folders |
//! | `finished`, one file | `list_finished_file.json` | `Ready`, one address, no folder walk |
//! | `seeding` | `list_seeding_folder.json` | `Ready`, exactly as `finished` |
//! | `error` | `list_error.json` | a refusal that ends the job |
//! | The sign-in expired | `error_not_logged_in.json` at HTTP **200** | `auth_invalid`, terminal |
//! | The request budget is spent | `error_rate_limit.json` + `Retry-After` | a wait carrying the header |
//! | The provider is down | `error_service_down.json` | a wait |
//! | The source is refused | `error_unsupported.json` | `source_unsupported` |
//! | Magnets are asked about the cache | `cache_check_magnet.json` | `cached`, `known` and `unknown`, one request (RD-130-11) |
//! | A cache query that is not a magnet | -- | `unknown`, no request |
//!
//! **A run against the real provider is not claimed here.** It needs a Premiumize account;
//! `docs/roadmap/jobs/120-23-premiumize-nimmt-auftraege-entgegen.md` records that as open.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_ext::{
    CacheKind, CacheQuery, CacheState, PollOutcome, RemoteJobRunners, StartOutcome,
};
use rd_plugin_host::{
    PluginManifest, PluginType,
    extension::{RemoteJobHandle, RemoteJobPlugin, RemoteJobSource},
};

const MANIFEST: &str = include_str!("../../../plugins/premiumize-transfers/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-00000000013f";

// The sanitised fixtures. Every identifier in them is a placeholder and every link points at
// a redacted path; `fixtures_carry_no_credential_material` is what keeps it that way.
const CREATE: &str = include_str!("fixtures/premiumize_transfers/create.json");
const LIST_QUEUED: &str = include_str!("fixtures/premiumize_transfers/list_queued.json");
const LIST_RUNNING: &str = include_str!("fixtures/premiumize_transfers/list_running.json");
const LIST_FINISHED_FOLDER: &str =
    include_str!("fixtures/premiumize_transfers/list_finished_folder.json");
const LIST_SEEDING_FOLDER: &str =
    include_str!("fixtures/premiumize_transfers/list_seeding_folder.json");
const LIST_FINISHED_FILE: &str =
    include_str!("fixtures/premiumize_transfers/list_finished_file.json");
const LIST_ERROR: &str = include_str!("fixtures/premiumize_transfers/list_error.json");
const LIST_UNKNOWN: &str = include_str!("fixtures/premiumize_transfers/list_unknown_state.json");
const LIST_WITHOUT_OURS: &str =
    include_str!("fixtures/premiumize_transfers/list_without_our_transfer.json");
const FOLDER_LIST: &str = include_str!("fixtures/premiumize_transfers/folder_list.json");
const FOLDER_LIST_SAMPLE: &str =
    include_str!("fixtures/premiumize_transfers/folder_list_sample.json");
const FOLDER_LIST_EMPTY: &str =
    include_str!("fixtures/premiumize_transfers/folder_list_empty.json");
const ITEM_DETAILS: &str = include_str!("fixtures/premiumize_transfers/item_details.json");
const ERROR_NOT_LOGGED_IN: &str =
    include_str!("fixtures/premiumize_transfers/error_not_logged_in.json");
const ERROR_RATE_LIMIT: &str = include_str!("fixtures/premiumize_transfers/error_rate_limit.json");
const ERROR_SERVICE_DOWN: &str =
    include_str!("fixtures/premiumize_transfers/error_service_down.json");
const ERROR_UNSUPPORTED: &str =
    include_str!("fixtures/premiumize_transfers/error_unsupported.json");
const DELETED: &str = include_str!("fixtures/premiumize_transfers/deleted.json");
const CACHE_CHECK: &str = include_str!("fixtures/premiumize_transfers/cache_check_magnet.json");

const TRANSFER_ID: &str = "REDACTEDTRANSFER01";
const FOLDER_ID: &str = "REDACTEDFOLDER01";
const SUB_FOLDER_ID: &str = "REDACTEDSUB0001";
const FILE_ID: &str = "REDACTEDFILE01";
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
const MAGNET_KEY: &str = "btih:da39a3ee5e6b4b0d3255bfef95601890afd80709";
const ADDRESS: &str = "https://example.invalid/some/file.bin";
const API: &str = "/api";
const KEY_TEMPLATE: &str = "Bearer {{secret:premiumize_api_key}}";
const TORRENT: &[u8] = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee";

/// The plugin component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-premiumize-transfers")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// One request the plugin made, flattened to what a test asserts on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Recorded {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The mock Premiumize API, routed by method and path.
struct MockPremiumize {
    /// What `transfer/list` answers next, in order; the last one repeats.
    list: Mutex<VecDeque<&'static str>>,
    /// What `folder/list` answers for the transfer's own folder.
    folder: &'static str,
    /// When set, every request is answered with this status, body and `Retry-After`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockPremiumize {
    fn new(list: &[&'static str]) -> Arc<Self> {
        Self::with_folder(list, FOLDER_LIST)
    }

    fn with_folder(list: &[&'static str], folder: &'static str) -> Arc<Self> {
        Arc::new(Self {
            list: Mutex::new(list.iter().copied().collect()),
            folder,
            failure: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, retry_after: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            list: Mutex::new(VecDeque::new()),
            folder: FOLDER_LIST,
            failure: Some((status, body, retry_after)),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }

    fn next_list(&self) -> &'static str {
        let mut queue = self.list.lock().expect("list");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(LIST_QUEUED)
        }
    }
}

#[async_trait]
impl ResolverHost for MockPremiumize {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let mut content_type = None;
        for header in &request.headers {
            if header.name.eq_ignore_ascii_case("authorization") {
                self.authorizations
                    .lock()
                    .expect("authorizations")
                    .push(header.value_template.clone());
            }
            if header.name.eq_ignore_ascii_case("content-type") {
                content_type = Some(header.value_template.clone());
            }
        }
        let path = request.url.path().to_owned();
        let query: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            path: path.clone(),
            query: query.clone(),
            content_type,
            body: request.body.clone(),
        });
        let answer = |status: u16, body: &str, retry_after: Option<&str>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: retry_after
                    .map(|value| ResolvedHeader {
                        name: "Retry-After".to_owned(),
                        value: value.to_owned(),
                    })
                    .into_iter()
                    .collect(),
                body: body.as_bytes().to_vec(),
            })
        };
        if let Some((status, body, retry_after)) = self.failure {
            return answer(status, body, retry_after);
        }
        let asked_for = |name: &str| {
            query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{API}/transfer/create") => answer(200, CREATE, None),
            ("GET", p) if p == format!("{API}/transfer/list") => {
                answer(200, self.next_list(), None)
            }
            ("POST", p) if p == format!("{API}/transfer/delete") => answer(200, DELETED, None),
            ("POST", p) if p == format!("{API}/cache/check") => answer(200, CACHE_CHECK, None),
            ("GET", p) if p == format!("{API}/folder/list") => match asked_for("id").as_deref() {
                Some(FOLDER_ID) => answer(200, self.folder, None),
                Some(SUB_FOLDER_ID) => answer(200, FOLDER_LIST_SAMPLE, None),
                _ => answer(200, r#"{"status":"error","code":"not_found"}"#, None),
            },
            ("GET", p) if p == format!("{API}/item/details") => match asked_for("id").as_deref() {
                Some(FILE_ID) => answer(200, ITEM_DETAILS, None),
                _ => answer(200, r#"{"status":"error","code":"not_found"}"#, None),
            },
            // The one call this plugin must never make, answered so that a test which reached
            // it would see the request rather than a 404 it could mistake for a routing slip.
            ("POST", p) if p == format!("{API}/transfer/clearfinished") => {
                answer(200, DELETED, None)
            }
            _ => answer(404, r#"{"status":"error","code":"not_found"}"#, None),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "premiumize_api_key"
    }
}

fn runners_for(host: Arc<MockPremiumize>, bytes: &[u8]) -> RemoteJobRunners {
    let plugin = RemoteJobPlugin::new(manifest(), bytes, Some(host)).expect("the plugin builds");
    RemoteJobRunners::from_plugins(vec![plugin])
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

fn handle() -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: TRANSFER_ID.to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: None,
    }
}

#[tokio::test]
async fn the_plugin_compiles_against_the_remote_job_world() {
    let bytes = component();
    RemoteJobPlugin::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
}

/// The two calls that reach nothing, over all three shapes of source -- which is where this
/// plugin goes past `plugins/realdebrid-torrents/`: `src` takes a plain address and a
/// container as readily as a magnet, so all three are claimed and all three get a key.
#[tokio::test]
async fn all_three_shapes_of_source_are_claimed_and_keyed_without_a_request() {
    let bytes = component();
    let host = MockPremiumize::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);

    // One torrent, two spellings of its hash, one key.
    for spelling in [MAGNET, MAGNET_BASE32] {
        assert_eq!(
            runners
                .identify("premiumize", &RemoteJobSource::Magnet(spelling.to_owned()))
                .await,
            StartOutcome::Identified {
                plugin_id: PLUGIN_ID.to_owned(),
                content_key: MAGNET_KEY.to_owned(),
            },
            "{spelling}"
        );
    }

    // A plain address: the third shape of `job-source` (RD-120-20), and the one Real-Debrid's
    // torrent endpoints have no use for.
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("premiumize", &RemoteJobSource::Address(ADDRESS.to_owned()))
        .await
    else {
        panic!("a plain address is claimed");
    };
    assert!(content_key.starts_with("url:"), "{content_key}");

    // A container: keyed by its bytes, the same twice over.
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("premiumize", &RemoteJobSource::Container(TORRENT.to_vec()))
        .await
    else {
        panic!("a container is claimed");
    };
    assert!(content_key.starts_with("file:"), "{content_key}");
    assert!(matches!(
        runners.identify("premiumize", &RemoteJobSource::Container(TORRENT.to_vec())).await,
        StartOutcome::Identified { content_key: again, .. } if again == content_key
    ));

    // What is not claimed: an address that is not fetchable, and a container whose format
    // cannot be named -- `.ccf` carries no marker, so it is refused rather than guessed at.
    for refused in [
        RemoteJobSource::Address("ftp://example.invalid/x".to_owned()),
        RemoteJobSource::Container(vec![0x9a; 64]),
    ] {
        assert_eq!(
            runners.identify("premiumize", &refused).await,
            StartOutcome::NotClaimed,
            "{refused:?}"
        );
    }

    assert!(
        host.requests().is_empty(),
        "claiming and identifying reach nothing"
    );
}

/// The whole way through, in the order the sweep drives it: submitted, queued, fetched, and
/// finished as addresses with names and folders the LinkGrabber can take.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_the_link_grabber_can_take() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_QUEUED, LIST_RUNNING, LIST_FINISHED_FOLDER]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), MAGNET_KEY)
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TRANSFER_ID);
    assert_eq!(handle.account_id, account.to_string());
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/transfer/create"));
    assert_eq!(
        submitted.content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert!(
        submitted
            .text()
            .starts_with("src=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{}",
        submitted.text()
    );

    // Queued: nothing is being fetched yet, so this is a wait and not a bar at zero percent.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(30)
        }
    );

    // Running: the provider's fraction, in thousandths. 0.425 is 425 and not 4.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(425));
    assert_eq!(work.speed_bytes_per_second, None);

    // Finished: three files across two folders, each rooted at the transfer's own name, and
    // the entry with no link dropped rather than guessed at.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    let mut named: Vec<(String, String)> = artifacts
        .iter()
        .map(|artifact| {
            (
                artifact.file_name.clone().unwrap_or_default(),
                artifact.package_hint.clone().unwrap_or_default(),
            )
        })
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            ("ep01.mkv".to_owned(), "Example.Release".to_owned()),
            ("ep02.mkv".to_owned(), "Example.Release".to_owned()),
            ("sample.mkv".to_owned(), "Example.Release/Sample".to_owned()),
        ]
    );
    let first = artifacts
        .iter()
        .find(|artifact| artifact.file_name.as_deref() == Some("ep01.mkv"))
        .expect("ep01");
    assert_eq!(
        first.url.as_str(),
        "https://8.premiumize.me/dl/REDACTEDFILE01/ep01.mkv"
    );
    assert_eq!(first.size, Some(10), "a size quoted as a string is read");

    // Nothing in the chain deleted anything, and `clearfinished` was never reached.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.ends_with("transfer/delete")
                && !request.path.ends_with("clearfinished")),
        "discard is never a side effect, and clearfinished is never called at all"
    );
}

/// A container goes out as the upload, not as a stringified URI: there is no address to put
/// in `src`, and the file name is what tells Premiumize which format it is looking at.
#[tokio::test]
async fn a_container_is_submitted_as_a_multipart_upload_that_names_its_format() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_QUEUED]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("premiumize", &RemoteJobSource::Container(TORRENT.to_vec()))
        .await
    else {
        panic!("a container is claimed");
    };
    let handle = runners
        .submit(
            PLUGIN_ID,
            account,
            &RemoteJobSource::Container(TORRENT.to_vec()),
            &content_key,
        )
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TRANSFER_ID);

    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/transfer/create"));
    let content_type = submitted.content_type.clone().expect("a content type");
    assert!(
        content_type.starts_with("multipart/form-data; boundary=----rdownloader"),
        "{content_type}"
    );
    let boundary = content_type
        .rsplit_once("boundary=")
        .expect("a boundary")
        .1
        .to_owned();
    let text = submitted.text();
    assert!(text.starts_with(&format!("--{boundary}\r\n")), "{text}");
    assert!(
        text.contains(r#"Content-Disposition: form-data; name="src"; filename="source.torrent""#),
        "{text}"
    );
    assert!(
        text.contains("8:announce"),
        "the container's own bytes travel, not a rendering of them"
    );
    assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")), "{text}");

    // A DLC is named as one, because Premiumize treats it differently from a torrent.
    let dlc = b"UEsDBBQAAAAIAA+dtVYAAAAA/w==UEsDBBQAAAAIAA==".to_vec();
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("premiumize", &RemoteJobSource::Container(dlc.clone()))
        .await
    else {
        panic!("a dlc is claimed");
    };
    runners
        .submit(
            PLUGIN_ID,
            account,
            &RemoteJobSource::Container(dlc),
            &content_key,
        )
        .await
        .expect("submitted");
    assert!(
        host.requests()
            .last()
            .expect("the second submit")
            .text()
            .contains(r#"filename="source.dlc""#)
    );
}

/// A plain address travels as a URI in a form body -- the flavour `src` takes for everything
/// that is not a file.
#[tokio::test]
async fn a_plain_address_is_submitted_as_a_uri() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_QUEUED]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("premiumize", &RemoteJobSource::Address(ADDRESS.to_owned()))
        .await
    else {
        panic!("an address is claimed");
    };
    runners
        .submit(
            PLUGIN_ID,
            account,
            &RemoteJobSource::Address(ADDRESS.to_owned()),
            &content_key,
        )
        .await
        .expect("submitted");
    let submitted = &host.requests()[0];
    assert_eq!(
        submitted.content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        submitted.text(),
        "src=https%3A%2F%2Fexample.invalid%2Fsome%2Ffile.bin"
    );
}

/// All five values of the measurement, each on one state of the contract, plus the word the
/// measurement did not contain.
#[tokio::test]
async fn every_status_of_the_measurement_reaches_its_own_state() {
    let bytes = component();
    let account = AccountId::new();

    // `queued`, `running` and `error` need nothing but the list.
    for (fixture, expected) in [
        (
            LIST_QUEUED,
            PollOutcome::Preparing {
                retry_after_seconds: Some(30),
            },
        ),
        (
            LIST_UNKNOWN,
            PollOutcome::Preparing {
                retry_after_seconds: Some(60),
            },
        ),
    ] {
        let host = MockPremiumize::new(&[fixture]);
        let runners = runners_for(host, &bytes);
        assert_eq!(runners.poll(PLUGIN_ID, account, &handle()).await, expected);
    }

    let host = MockPremiumize::new(&[LIST_RUNNING]);
    let runners = runners_for(host, &bytes);
    assert!(matches!(
        runners.poll(PLUGIN_ID, account, &handle()).await,
        PollOutcome::Working(work) if work.progress_permille == Some(425)
    ));

    let host = MockPremiumize::new(&[LIST_ERROR]);
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("`error` ends the job");
    };
    assert_eq!(refusal.code, "premiumize_transfers.transfer_failed");
    assert!(!refusal.retryable);
    assert!(
        !refusal.message.contains("could not fetch the source"),
        "the provider's sentence does not travel: {}",
        refusal.message
    );

    // `finished` and `seeding` both hand the addresses over, and hand over the same ones.
    let mut ready = Vec::new();
    for fixture in [LIST_FINISHED_FOLDER, LIST_SEEDING_FOLDER] {
        let host = MockPremiumize::new(&[fixture]);
        let runners = runners_for(host, &bytes);
        let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle()).await
        else {
            panic!("expected the addresses");
        };
        let mut names: Vec<String> = artifacts
            .iter()
            .filter_map(|artifact| artifact.file_name.clone())
            .collect();
        names.sort();
        ready.push(names);
    }
    assert_eq!(ready[0], ready[1], "seeding is ready, exactly as finished");
    assert_eq!(ready[0].len(), 3);
}

/// `seeding` with nothing to hand over yet is a wait; `finished` with nothing is a failure.
/// The one difference between the two, and the reason `Stage::Ready` carries a flag at all.
#[tokio::test]
async fn an_empty_answer_is_a_wait_while_seeding_and_a_failure_once_finished() {
    let bytes = component();
    let account = AccountId::new();

    let host = MockPremiumize::with_folder(&[LIST_SEEDING_FOLDER], FOLDER_LIST_EMPTY);
    let runners = runners_for(host, &bytes);
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle()).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(60)
        },
        "the transfer is still running, so there is something to wait for"
    );

    let host = MockPremiumize::with_folder(&[LIST_FINISHED_FOLDER], FOLDER_LIST_EMPTY);
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("finished with nothing is not a result");
    };
    assert_eq!(refusal.code, "premiumize_transfers.no_files");
    assert!(!refusal.retryable);
}

/// A transfer holding one file names it in `file_id`, and then no folder is walked at all.
/// A transfer holding several leaves `file_id` null, and the folder is what says what is in
/// it -- which is the whole of the `file_id` semantics the measurement recorded.
#[tokio::test]
async fn one_file_is_read_from_its_item_and_several_from_the_folder() {
    let bytes = component();
    let account = AccountId::new();

    let host = MockPremiumize::new(&[LIST_FINISHED_FILE]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected one address");
    };
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("one.mkv"));
    assert_eq!(artifacts[0].size, Some(4096));
    assert_eq!(artifacts[0].package_hint.as_deref(), Some("one.mkv"));
    let paths: Vec<String> = host
        .requests()
        .iter()
        .map(|request| request.path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            format!("{API}/transfer/list"),
            format!("{API}/item/details"),
        ],
        "a single file costs one lookup and no folder walk"
    );
    assert_eq!(
        host.requests()[1].query,
        vec![("id".to_owned(), FILE_ID.to_owned())],
        "the item is asked for by the id the transfer named"
    );

    let host = MockPremiumize::new(&[LIST_FINISHED_FOLDER]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected several addresses");
    };
    assert_eq!(artifacts.len(), 3);
    assert!(
        host.requests()
            .iter()
            .any(|request| request.path.ends_with("folder/list"))
    );
}

/// The key the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_api_key_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_FINISHED_FILE]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), MAGNET_KEY)
        .await
        .expect("submitted");
    let _ = runners.poll(PLUGIN_ID, account, &handle).await;
    runners
        .discard(PLUGIN_ID, account, &handle)
        .await
        .expect("discarded");
    let authorizations = host.authorizations();
    assert_eq!(authorizations.len(), 4, "{authorizations:?}");
    assert!(
        authorizations.iter().all(|value| value == KEY_TEMPLATE),
        "{authorizations:?}"
    );
    // And nothing else on the wire names the reference either.
    for request in host.requests() {
        assert!(
            !request.text().contains("premiumize_api_key"),
            "{request:?}"
        );
    }
}

/// The measurement's own finding, as a test: Premiumize reports a refusal with HTTP `200`.
/// A reader that believed the status line would take `Not logged in` for a success.
#[tokio::test]
async fn an_error_answered_with_http_200_is_still_an_error() {
    let bytes = component();
    let host = MockPremiumize::failing(200, ERROR_NOT_LOGGED_IN, None);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "premiumize_transfers.auth_invalid");
    assert!(!refusal.retryable, "waiting does not bring a sign-in back");
    assert!(
        !refusal.message.contains("Not logged in"),
        "the provider's sentence does not travel: {}",
        refusal.message
    );
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), MAGNET_KEY)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "premiumize_transfers.auth_invalid");
    assert!(!refusal.retryable);
}

/// The other three shapes of refusal the fixtures carry, each in its own bucket.
#[tokio::test]
async fn a_rate_limit_waits_an_outage_waits_and_a_refused_source_does_not() {
    let bytes = component();
    let account = AccountId::new();

    // The budget is spent, and the provider's own figure is carried out for the host to clamp.
    let host = MockPremiumize::failing(200, ERROR_RATE_LIMIT, Some("120"));
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "premiumize_transfers.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));

    // An outage waits too, on the bucket's own figure rather than on a header.
    let host = MockPremiumize::failing(200, ERROR_SERVICE_DOWN, None);
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "premiumize_transfers.server_busy");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(300));

    // A source the account's plan will not take is not a wait: asking again changes nothing.
    let host = MockPremiumize::failing(200, ERROR_UNSUPPORTED, None);
    let runners = runners_for(host, &bytes);
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), MAGNET_KEY)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "premiumize_transfers.source_unsupported");
    assert!(!refusal.retryable);

    // And a gateway page with no envelope at all is classified by its status alone.
    let host = MockPremiumize::failing(503, "<html>502</html>", None);
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "premiumize_transfers.http_error");
    assert!(refusal.retryable);
}

/// A transfer the account no longer lists is gone, and it is not mistaken for somebody
/// else's: the list is looked up by id and a stranger's row is not adopted into this job.
#[tokio::test]
async fn a_transfer_the_account_no_longer_lists_is_gone() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_WITHOUT_OURS]);
    let runners = runners_for(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "premiumize_transfers.transfer_unlisted");
    assert!(!refusal.retryable);
}

/// `adopt` answers `none` and reaches nothing, and that is a decision rather than an
/// omission: `transfer/list` carries nothing derived from what was handed over, so a
/// transfer this installation created cannot be told from a stranger's. Guessing would poll
/// somebody else's transfer as this job.
#[tokio::test]
async fn adoption_answers_none_without_asking_the_provider() {
    let bytes = component();
    let host = MockPremiumize::new(&[LIST_FINISHED_FOLDER]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    assert_eq!(
        runners
            .adopt(PLUGIN_ID, AccountId::new(), MAGNET_KEY)
            .await
            .expect("answered"),
        None
    );
    assert!(host.requests().is_empty(), "adoption asks nothing");
}

/// This plugin asks no question, so an answer to one is refused rather than silently
/// accepted: Premiumize has no call that tells a transfer which files to keep, and reporting
/// a selection that changed nothing would be reporting work that did not happen.
#[tokio::test]
async fn a_selection_is_refused_because_the_provider_has_none() {
    let bytes = component();
    let host = MockPremiumize::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let refusal = runners
        .choose(PLUGIN_ID, account, &handle(), &[1, 2])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "premiumize_transfers.no_choice");
    // The host refuses an empty one before it ever reaches the guest.
    let refusal = runners
        .choose(PLUGIN_ID, account, &handle(), &[])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "remote_job.empty_choice");
    assert!(host.requests().is_empty());
}

/// `discard` reaches `transfer/delete` exactly when it is called, and never
/// `transfer/clearfinished` -- a collective call that would delete transfers this
/// installation never created.
#[tokio::test]
async fn discard_deletes_one_transfer_and_never_clears_the_account() {
    let bytes = component();
    let host = MockPremiumize::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("discarded");
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, format!("{API}/transfer/delete"));
    assert_eq!(requests[0].text(), format!("id={TRANSFER_ID}"));
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("clearfinished"))
    );
}

/// Nothing that could be a credential, an account's real identifier or a live link is
/// committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    fn walk(value: &serde_json::Value, path: &std::path::Path) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, inner) in fields {
                    assert!(
                        !matches!(
                            name.as_str(),
                            "apikey" | "api_key" | "access_token" | "token" | "customer_id"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    if matches!(name.as_str(), "id" | "folder_id" | "file_id")
                        && let Some(id) = inner.as_str()
                    {
                        assert!(id.starts_with("REDACTED"), "{path:?} carries a real id");
                    }
                    walk(inner, path);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, path);
                }
            }
            serde_json::Value::String(text) if text.starts_with("http") => {
                assert!(
                    text.contains("REDACTED"),
                    "{path:?} carries a live link: {text}"
                );
            }
            _ => {}
        }
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/premiumize_transfers");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 18, "only {checked} fixtures were checked");
}

fn torrent_query(magnet: &str) -> CacheQuery {
    CacheQuery {
        source: RemoteJobSource::Magnet(magnet.to_owned()),
        kind: CacheKind::Torrent,
    }
}

/// RD-130-11: magnets are asked about in one `cache/check`, and Premiumize's answers stay
/// apart -- `true` is cached, `false` with a name is known, anything else says nothing. A
/// hoster link in the same batch is not a kind this plugin names, so it never leaves the
/// adapter: the resolver already asks about hoster links during the link check.
#[tokio::test]
async fn a_cache_check_asks_about_magnets_in_one_request_and_keeps_known_apart() {
    let bytes = component();
    let host = MockPremiumize::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    assert_eq!(
        runners.cache_providers().await,
        vec![("premiumize".to_owned(), vec![CacheKind::Torrent])]
    );
    let queries = vec![
        torrent_query(MAGNET),
        CacheQuery {
            source: RemoteJobSource::Address(ADDRESS.to_owned()),
            kind: CacheKind::Hoster,
        },
        torrent_query("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Other"),
        torrent_query("magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef&dn=Third"),
    ];
    let answers = runners
        .check_cached("premiumize", AccountId::new(), &queries)
        .await
        .expect("answers");
    assert_eq!(answers.len(), 4, "one answer per query");
    assert_eq!(answers[0].state, CacheState::Cached);
    assert_eq!(answers[0].file_name.as_deref(), Some("Example.Release.mkv"));
    assert_eq!(answers[0].size, Some(1_048_576));
    assert_eq!(answers[1].state, CacheState::Unknown, "the hoster link");
    assert_eq!(answers[2].state, CacheState::Known);
    assert_eq!(answers[2].file_name.as_deref(), Some("Example.Other.mkv"));
    assert_eq!(answers[3].state, CacheState::Unknown);
    assert_eq!(answers[3].file_name, None);

    let requests = host.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, format!("{API}/cache/check"));
    assert_eq!(
        requests[0].content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    let body = requests[0].text();
    assert_eq!(body.matches("items%5B%5D=").count(), 3, "{body}");
    assert!(
        body.starts_with("items%5B%5D=magnet%3A%3Fxt%3Durn%3Abtih%3ADA39A3EE"),
        "the magnets go in the order they were asked: {body}"
    );
    assert!(!body.contains("example.invalid"), "{body}");
    assert_eq!(host.authorizations(), vec![KEY_TEMPLATE.to_owned()]);
}

/// A query this plugin cannot answer is `unknown`, and it costs no request -- asked of the
/// plugin itself, past the adapter that would have kept the wrong kinds home.
#[tokio::test]
async fn a_cache_query_that_is_not_a_magnet_is_unknown_without_a_request() {
    let bytes = component();
    let host = MockPremiumize::new(&[]);
    let plugin = RemoteJobPlugin::new(
        manifest(),
        &bytes,
        Some(Arc::clone(&host) as Arc<dyn ResolverHost>),
    )
    .expect("the plugin builds");
    let queries = [
        CacheQuery {
            source: RemoteJobSource::Address(ADDRESS.to_owned()),
            kind: CacheKind::Hoster,
        },
        CacheQuery {
            source: RemoteJobSource::Address(ADDRESS.to_owned()),
            kind: CacheKind::Usenet,
        },
        // A magnet asked as the wrong kind, and a torrent kind that is not a magnet.
        CacheQuery {
            source: RemoteJobSource::Magnet(MAGNET.to_owned()),
            kind: CacheKind::Usenet,
        },
        CacheQuery {
            source: RemoteJobSource::Container(TORRENT.to_vec()),
            kind: CacheKind::Torrent,
        },
    ];
    let answers = plugin
        .check_cached(AccountId::new(), &queries)
        .await
        .expect("the call")
        .expect("answers");
    assert_eq!(answers.len(), queries.len());
    assert!(
        answers
            .iter()
            .all(|answer| answer.state == CacheState::Unknown && answer.file_name.is_none()),
        "{answers:?}"
    );
    assert!(host.requests().is_empty(), "{:?}", host.requests());
}

/// A refusal of `cache/check` drops the whole call: no answer, and the refusal is the
/// plugin's usual one, not a list of `unknown` that would read as "asked, not held".
#[tokio::test]
async fn a_refused_cache_check_is_a_refusal_and_not_an_answer() {
    let bytes = component();
    let host = MockPremiumize::failing(200, ERROR_NOT_LOGGED_IN, None);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let refusal = runners
        .check_cached("premiumize", AccountId::new(), &[torrent_query(MAGNET)])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "premiumize_transfers.auth_invalid");
}

/// The plugin reaches only the API it needs, with only the key it needs, and claims the
/// provider row its three siblings already share.
#[test]
fn the_plugin_reaches_only_the_part_of_premiumize_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["premiumize".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["www.premiumize.me".to_owned(), "premiumize.me".to_owned()].as_slice(),
        "Premiumize's own domains and nothing else"
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["premiumize_api_key".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}
