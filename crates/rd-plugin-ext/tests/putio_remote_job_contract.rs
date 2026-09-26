//! The Put.io transfer contract, driven end to end against a mock of the provider
//! (RD-120-03).
//!
//! `plugins/putio-transfers/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-putio-transfers`
//! -- and the mock stands in for `api.put.io`. It answers at the host boundary, so no socket is
//! opened, no account is needed and no request leaves the machine; and because it sees each
//! request exactly as the plugin described it, a test can assert that the account's token left
//! the plugin as the template `{{secret:putio_access_token}}` and never as a value.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `transfer_added.json` | a handle carrying the transfer's id |
//! | Put.io has it queued | `transfer_in_queue.json` | `Preparing`, with a wait |
//! | Put.io is fetching | `transfer_downloading.json` | `Working`, 420 permille |
//! | The transfer finished | `transfer_completed.json` + the file tree | `Ready`, the whole tree |
//! | Put.io is still seeding | `transfer_seeding.json` | `Ready` all the same |
//! | A single-file torrent | `file_root_single.json` | one address, no folder |
//! | Put.io ended it | `transfer_error.json`, `transfer_cancelled.json` | a refusal that ends the job |
//! | The sign-in expired | `error_invalid_token.json` | a refusal that ends the job, `auth_invalid` |
//! | The request budget is spent | `error_too_many_requests.json` + `X-RateLimit-Reset` | a wait |
//! | The account is full | `error_disk_quota.json` | a refusal naming the one thing to fix |
//! | A submit was lost in flight | `transfers_list.json` | adopted by its hash; a stranger's is not |
//!
//! **A run against the real provider is not claimed here.** It needs a Put.io account;
//! `docs/roadmap/jobs/archive/120-03-putio.md` records that as open.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};
use rd_plugin_ext::{PollOutcome, RemoteJobRunners, StartOutcome};
use rd_plugin_host::{
    PluginManifest, PluginType,
    extension::{RemoteJobHandle, RemoteJobPlugin, RemoteJobSource},
};

const MANIFEST: &str = include_str!("../../../plugins/putio-transfers/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-000000000152";

// The sanitised fixtures. Every identifier in them is a placeholder and every sentence Put.io
// would have written is the same redacted phrase; `fixtures_carry_no_credential_material` is
// what keeps it that way.
const ADDED: &str = include_str!("fixtures/putio_transfers/transfer_added.json");
const IN_QUEUE: &str = include_str!("fixtures/putio_transfers/transfer_in_queue.json");
const DOWNLOADING: &str = include_str!("fixtures/putio_transfers/transfer_downloading.json");
const COMPLETED: &str = include_str!("fixtures/putio_transfers/transfer_completed.json");
const SEEDING: &str = include_str!("fixtures/putio_transfers/transfer_seeding.json");
const ERRORED: &str = include_str!("fixtures/putio_transfers/transfer_error.json");
const CANCELLED: &str = include_str!("fixtures/putio_transfers/transfer_cancelled.json");
const LIST: &str = include_str!("fixtures/putio_transfers/transfers_list.json");
const ROOT_FOLDER: &str = include_str!("fixtures/putio_transfers/file_root_folder.json");
const ROOT_SINGLE: &str = include_str!("fixtures/putio_transfers/file_root_single.json");
const FILES_ROOT: &str = include_str!("fixtures/putio_transfers/files_root.json");
const FILES_SAMPLE: &str = include_str!("fixtures/putio_transfers/files_sample.json");
const ERROR_TOKEN: &str = include_str!("fixtures/putio_transfers/error_invalid_token.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/putio_transfers/error_too_many_requests.json");
const ERROR_DISK: &str = include_str!("fixtures/putio_transfers/error_disk_quota.json");

/// The info hash every fixture carries: SHA-1 of nothing, recognisable as a placeholder.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const TRANSFER_ID: &str = "770001";
/// The folder `file_root_folder.json` stands for, and the single file of the other root.
const ROOT_FOLDER_ID: u64 = 900_001;
const SINGLE_FILE_ID: u64 = 900_009;
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
const API: &str = "/v2";
const TOKEN_TEMPLATE: &str = "Bearer {{secret:putio_access_token}}";

/// The plugin component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-putio-transfers")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// A bencoded `.torrent` whose lengths are computed rather than counted out by hand.
fn container(name: &str) -> Vec<u8> {
    fn bencode(value: &str) -> String {
        format!("{}:{value}", value.len())
    }
    let announce = bencode("http://tracker.invalid/annce");
    format!(
        "d{}{announce}{}d{}i31e{}{}{}i16384e{}{}ee",
        bencode("announce"),
        bencode("info"),
        bencode("length"),
        bencode("name"),
        bencode(name),
        bencode("piece length"),
        bencode("pieces"),
        bencode("01234567890123456789"),
    )
    .into_bytes()
}

/// One request the plugin made, flattened to what a test asserts on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: String,
}

/// What the mock host's clock always reads: 2026-09-25, a fixed instant rather than the wall
/// clock, so a wait computed from it is exact.
const MOCK_NOW: u64 = 1_790_294_400;

/// The mock Put.io API, routed by method and path.
struct MockPutio {
    /// What `transfers/{id}` answers next, in order; the last one repeats.
    transfers: Mutex<VecDeque<&'static str>>,
    /// What `files/{root}` answers: the folder tree, or the one loose file.
    root: &'static str,
    /// When set, every request is answered with this status, body and `X-RateLimit-Reset`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockPutio {
    fn new(transfers: &[&'static str]) -> Arc<Self> {
        Self::with_root(transfers, ROOT_FOLDER)
    }

    fn with_root(transfers: &[&'static str], root: &'static str) -> Arc<Self> {
        Arc::new(Self {
            transfers: Mutex::new(transfers.iter().copied().collect()),
            root,
            failure: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, reset: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            transfers: Mutex::new(VecDeque::new()),
            root: ROOT_FOLDER,
            failure: Some((status, body, reset)),
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

    fn next_transfer(&self) -> &'static str {
        let mut queue = self.transfers.lock().expect("transfers");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(IN_QUEUE)
        }
    }
}

#[async_trait]
impl ResolverHost for MockPutio {
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
            body: String::from_utf8_lossy(&request.body).into_owned(),
        });
        let answer = |status: u16, body: &str, reset: Option<&str>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: reset
                    .map(|value| ResolvedHeader {
                        name: "X-RateLimit-Reset".to_owned(),
                        value: value.to_owned(),
                    })
                    .into_iter()
                    .collect(),
                body: body.as_bytes().to_vec(),
            })
        };
        if let Some((status, body, reset)) = self.failure {
            return answer(status, body, reset);
        }
        let parent = query
            .iter()
            .find(|(name, _)| name == "parent_id")
            .map(|(_, value)| value.as_str());
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{API}/transfers/add") => answer(200, ADDED, None),
            ("GET", p) if p == format!("{API}/transfers/list") => answer(200, LIST, None),
            ("GET", p) if p == format!("{API}/transfers/{TRANSFER_ID}") => {
                answer(200, self.next_transfer(), None)
            }
            ("POST", p) if p == format!("{API}/transfers/cancel") => {
                answer(200, r#"{"status":"OK"}"#, None)
            }
            ("GET", p) if p == format!("{API}/files/{ROOT_FOLDER_ID}") => {
                answer(200, self.root, None)
            }
            ("GET", p) if p == format!("{API}/files/{SINGLE_FILE_ID}") => {
                answer(200, ROOT_SINGLE, None)
            }
            ("GET", p) if p == format!("{API}/files/list") => match parent {
                Some("900001") => answer(200, FILES_ROOT, None),
                Some("900004") => answer(200, FILES_SAMPLE, None),
                _ => answer(200, r#"{"files":[],"status":"OK"}"#, None),
            },
            _ => answer(
                404,
                r#"{"error_type":"NOT_FOUND","error_message":"redacted provider sentence"}"#,
                None,
            ),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "putio_access_token"
    }

    fn now_unix_seconds(&self) -> u64 {
        MOCK_NOW
    }
}

fn runners_for(host: Arc<MockPutio>, bytes: &[u8]) -> RemoteJobRunners {
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

/// RD-130-11: no cache query is bound for put.io, so the plugin names no cache kind and
/// the host never asks it. Asked all the same, it answers `unknown` -- one answer per query --
/// and without a request: there is no host here a request could reach.
#[tokio::test]
async fn the_plugin_names_no_cache_kind_and_answers_unknown_without_a_request() {
    let bytes = component();
    let plugin =
        RemoteJobPlugin::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
    assert!(plugin.cache_kinds().await.expect("the call").is_empty());
    let query = rd_plugin_ext::CacheQuery {
        source: magnet(),
        kind: rd_plugin_ext::CacheKind::Torrent,
    };
    let answers = plugin
        .check_cached(rd_core::AccountId::new(), &[query.clone(), query])
        .await
        .expect("the call")
        .expect("answers");
    assert_eq!(answers, vec![rd_plugin_ext::CacheAnswer::unknown(); 2]);
}

/// The two calls that reach nothing: whether the source is a torrent at all, and the key it is
/// known by. Both spellings of one info hash are one key, which is what makes a magnet copied
/// from two sites one job and not two.
#[tokio::test]
async fn only_torrent_sources_are_claimed_and_the_key_is_the_info_hash() {
    let bytes = component();
    let host = MockPutio::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);

    for spelling in [MAGNET, MAGNET_BASE32] {
        assert_eq!(
            runners
                .identify("putio", &RemoteJobSource::Magnet(spelling.to_owned()))
                .await,
            StartOutcome::Identified {
                plugin_id: PLUGIN_ID.to_owned(),
                content_key: HASH.to_owned(),
            },
            "{spelling}"
        );
    }
    // A hoster link is the resolver's business, and a magnet naming no BitTorrent hash is
    // nobody's.
    for foreign in [
        "https://api.put.io/v2/files/900002/download",
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
    ] {
        assert_eq!(
            runners
                .identify("putio", &RemoteJobSource::Magnet(foreign.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{foreign}"
        );
    }
    // A container is keyed by the same rule: the SHA-1 of its `info` dictionary, forty hex
    // digits, the same twice over.
    let torrent = container("Example.Release");
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("putio", &RemoteJobSource::Container(torrent.clone()))
        .await
    else {
        panic!("a torrent file is claimed");
    };
    assert_eq!(content_key.len(), 40);
    assert!(content_key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(matches!(
        runners.identify("putio", &RemoteJobSource::Container(torrent)).await,
        StartOutcome::Identified { content_key: again, .. } if again == content_key
    ));
    // The third shape of source (RD-120-20). Put.io would fetch an ordinary address, but an
    // address carries no info hash and therefore no key that could keep a restart from
    // submitting it twice, so this plugin does not claim one.
    assert_eq!(
        runners
            .identify(
                "putio",
                &RemoteJobSource::Address("https://example.invalid/some/file.bin".to_owned())
            )
            .await,
        StartOutcome::NotClaimed
    );
    assert!(
        host.requests().is_empty(),
        "claiming and identifying reach nothing"
    );
}

/// The whole way through, in the order the sweep drives it: submitted, queued, fetched, and
/// finished as addresses with names, sizes and the folders they sat in.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_the_link_grabber_can_take() {
    let bytes = component();
    let host = MockPutio::new(&[IN_QUEUE, DOWNLOADING, COMPLETED]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Submit: one request, a form body carrying the magnet, and the id Put.io named.
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TRANSFER_ID);
    assert_eq!(handle.account_id, account.to_string());
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/transfers/add"));
    assert!(
        submitted
            .body
            .starts_with("url=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{}",
        submitted.body
    );

    // Queued: nobody is needed, and the plugin suggests a wait the host will clamp.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(30)
        }
    );

    // Fetching: Put.io's percent, in thousandths, with its own speed and estimate.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(420));
    assert_eq!(work.speed_bytes_per_second, Some(1_048_576));

    // Finished: the whole tree, every file with its name, its size and its folder -- and the
    // nested one under the folder it actually sat in rather than the previous file's.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 3);
    assert_eq!(
        artifacts[0].url.as_str(),
        "https://api.put.io/v2/files/900002/download"
    );
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[0].size, Some(10));
    assert_eq!(
        artifacts[0].package_hint.as_deref(),
        Some("Example.Release")
    );
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ep02.mkv"));
    assert_eq!(artifacts[1].size, Some(20));
    assert_eq!(
        artifacts[2].package_hint.as_deref(),
        Some("Example.Release/Sample"),
        "a nested file keeps its own folder"
    );

    // The walk asked for one page per folder, at Put.io's own maximum, and for the two folders
    // the transfer actually has -- not for a third.
    let listings: Vec<Vec<(String, String)>> = host
        .requests()
        .into_iter()
        .filter(|request| request.path == format!("{API}/files/list"))
        .map(|request| request.query)
        .collect();
    assert_eq!(
        listings,
        vec![
            vec![
                ("parent_id".to_owned(), "900001".to_owned()),
                ("per_page".to_owned(), "1000".to_owned()),
            ],
            vec![
                ("parent_id".to_owned(), "900004".to_owned()),
                ("per_page".to_owned(), "1000".to_owned()),
            ],
        ]
    );

    // Nothing in the chain cancelled anything at the provider.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.ends_with("/transfers/cancel")),
        "discard is never a side effect"
    );
}

/// The addresses handed over are the stable per-file ones, never a signed short-lived URL and
/// never one carrying a credential. That is what makes a job that waits an hour in the queue
/// still downloadable, and it is why the plugin never calls `files/{id}/url` at all.
#[tokio::test]
async fn the_addresses_handed_over_expire_with_nothing_and_carry_no_credential() {
    let bytes = component();
    let host = MockPutio::new(&[COMPLETED]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected the addresses");
    };
    for artifact in &artifacts {
        let url = artifact.url.as_str();
        assert!(url.starts_with("https://api.put.io/v2/files/"), "{url}");
        assert!(url.ends_with("/download"), "{url}");
        assert!(artifact.url.query().is_none(), "{url}");
        assert!(!url.contains("token"), "{url}");
    }
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.ends_with("/url")),
        "a short-lived address is never asked for"
    );
}

/// Seeding is finished. Put.io giving back to the swarm is its business, not a reason to make
/// somebody wait for files that are already complete.
#[tokio::test]
async fn a_seeding_transfer_is_ready_and_not_a_wait() {
    let bytes = component();
    let runners = runners_for(MockPutio::new(&[SEEDING]), &bytes);
    assert!(matches!(
        runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await,
        PollOutcome::Ready(artifacts) if artifacts.len() == 3
    ));
}

/// A torrent of one file: Put.io names the file itself rather than a folder, so there is no
/// folder to put it in and the plugin does not invent one.
#[tokio::test]
async fn a_single_file_transfer_hands_over_one_address_and_no_folder() {
    let bytes = component();
    let single = r#"{"transfer":{"id":770001,"name":"loose.mkv","status":"COMPLETED",
        "percent_done":100,"file_id":900009}}"#;
    let host = MockPutio::with_root(&[single], ROOT_SINGLE);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected the address");
    };
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("loose.mkv"));
    assert_eq!(artifacts[0].size, Some(7));
    assert_eq!(artifacts[0].package_hint, None);
    // One `files/{id}` and no listing at all: a file is not a folder to walk.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.ends_with("/files/list"))
    );
}

/// The token the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockPutio::new(&[IN_QUEUE]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect("submitted");
    let _ = runners.poll(PLUGIN_ID, account, &handle).await;
    let _ = runners.adopt(PLUGIN_ID, account, HASH).await;
    let authorizations = host.authorizations();
    assert_eq!(authorizations.len(), 3);
    assert!(
        authorizations.iter().all(|value| value == TOKEN_TEMPLATE),
        "{authorizations:?}"
    );
}

/// A transfer Put.io ended is a refusal that ends the job, under the code that says how.
#[tokio::test]
async fn a_transfer_the_provider_ended_is_a_failure_and_not_a_wait() {
    let bytes = component();
    for (fixture, code) in [
        (ERRORED, "putio_transfers.transfer_failed"),
        (CANCELLED, "putio_transfers.transfer_cancelled"),
    ] {
        let runners = runners_for(MockPutio::new(&[fixture]), &bytes);
        let PollOutcome::Refused(refusal) =
            runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
        else {
            panic!("expected a refusal for {code}");
        };
        assert_eq!(refusal.code, code);
        assert!(!refusal.retryable, "{code}");
        // Put.io's own sentence about the failure is nowhere in what travels.
        assert!(!refusal.message.contains("redacted provider sentence"));
    }
}

/// The sign-in is gone. Waiting does not bring it back, so the job ends and the person is told
/// to sign in again rather than watching a job poll for ever.
#[tokio::test]
async fn an_expired_sign_in_ends_the_job_under_its_own_code() {
    let bytes = component();
    let runners = runners_for(MockPutio::failing(401, ERROR_TOKEN, None), &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "putio_transfers.auth_invalid");
    assert!(!refusal.retryable);
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "putio_transfers.auth_invalid");
    assert!(!refusal.retryable);
}

/// A spent request budget is a wait, and the wait is the window Put.io named: its
/// `X-RateLimit-Reset` is an absolute timestamp, so the plugin turns it into a duration with
/// the host's own clock rather than guessing.
///
/// The mock host's clock stands still at `MOCK_NOW`, so the answer is exact. It used to be
/// measured against the wall clock with a five-second margin, and a GitHub runner that took
/// six seconds to compile the component between reading the clock and polling missed it
/// (RD-120-67).
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_put_ios_own_window() {
    let bytes = component();
    // Inside the hour this plugin believes a reset for, and different from the default minute
    // so the two cannot be confused.
    let reset: &'static str = Box::leak((MOCK_NOW + 300).to_string().into_boxed_str());
    let runners = runners_for(MockPutio::failing(429, ERROR_TOO_MANY, Some(reset)), &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "putio_transfers.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(300));

    // And without a header the plugin waits its own minute rather than inventing one.
    let without_header = runners_for(MockPutio::failing(429, ERROR_TOO_MANY, None), &bytes);
    let PollOutcome::Refused(refusal) = without_header
        .poll(PLUGIN_ID, AccountId::new(), &handle())
        .await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.retry_after_seconds, Some(60));
    assert!(refusal.retryable);
}

/// A full account is the one refusal a person can act on straight away, so it says so rather
/// than arriving as "Put.io said no".
#[tokio::test]
async fn a_full_account_ends_the_job_naming_the_one_thing_to_fix() {
    let bytes = component();
    let runners = runners_for(MockPutio::failing(400, ERROR_DISK, None), &bytes);
    let refusal = runners
        .submit(PLUGIN_ID, AccountId::new(), &magnet(), HASH)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "putio_transfers.disk_full");
    assert!(!refusal.retryable);
}

/// The crash window, closed at the provider: a submit whose answer never arrived is found again
/// by its hash in the account's own list, and somebody else's transfer is not.
///
/// This is the restart half of the idempotency argument at the plugin's end. The other half --
/// the row written before the request goes out, and the unique key on it -- is the host's, and
/// `crates/rd-api/src/remote_job_service/tests.rs` drives that.
#[tokio::test]
async fn an_orphaned_transfer_is_adopted_by_its_hash_and_a_stranger_is_not() {
    let bytes = component();
    let host = MockPutio::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let adopted = runners
        .adopt(PLUGIN_ID, account, HASH)
        .await
        .expect("listed")
        .expect("found by its hash");
    assert_eq!(adopted.remote_id, TRANSFER_ID);
    let listed = &host.requests()[0];
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, format!("{API}/transfers/list"));
    assert_eq!(
        runners
            .adopt(
                PLUGIN_ID,
                account,
                "ffffffffffffffffffffffffffffffffffffffff"
            )
            .await
            .expect("listed")
            .map(|handle| handle.remote_id),
        Some("770002".to_owned()),
        "the stranger's hash is the stranger's transfer and nothing else"
    );
    assert_eq!(
        runners
            .adopt(
                PLUGIN_ID,
                account,
                "0000000000000000000000000000000000000000"
            )
            .await
            .expect("listed"),
        None,
        "a hash nobody holds is adopted by nobody"
    );
}

/// A restart in the submit window costs nothing at the provider: the same magnet identifies the
/// same key, and the key finds the transfer that already exists.
#[tokio::test]
async fn a_restart_finds_the_same_key_and_the_transfer_it_already_created() {
    let bytes = component();
    let host = MockPutio::new(&[IN_QUEUE]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Before the crash: one submit, one transfer.
    let first = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect("submitted");

    // After it: the process is new, the plugin remembers nothing, and the same source produces
    // the same key -- which is the whole reason the key is derived without a request.
    let restarted = runners_for(Arc::clone(&host), &bytes);
    assert_eq!(
        restarted.identify("putio", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: HASH.to_owned(),
        }
    );
    let adopted = restarted
        .adopt(PLUGIN_ID, account, HASH)
        .await
        .expect("listed")
        .expect("the transfer the first submit created");
    assert_eq!(adopted.remote_id, first.remote_id);
    assert_eq!(
        host.requests()
            .iter()
            .filter(|request| request.path.ends_with("/transfers/add"))
            .count(),
        1,
        "the restart submitted nothing a second time"
    );
}

/// A container and its magnet are one job, not two. That is the same claim the content key
/// makes, driven through the guest: the key is equal, and what the guest hands Put.io for the
/// container is a magnet naming exactly that hash.
#[tokio::test]
async fn a_torrent_file_and_its_magnet_are_one_job() {
    let bytes = component();
    let host = MockPutio::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let torrent = container("Example.Release");
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("putio", &RemoteJobSource::Container(torrent.clone()))
        .await
    else {
        panic!("a torrent file is claimed");
    };
    runners
        .submit(
            PLUGIN_ID,
            AccountId::new(),
            &RemoteJobSource::Container(torrent),
            &content_key,
        )
        .await
        .expect("submitted");
    let submitted = host.requests().last().cloned().expect("the submit");
    assert_eq!(submitted.path, format!("{API}/transfers/add"));
    // Put.io takes one address, so the container was handed over as the magnet it is
    // equivalent to -- carrying that same key, and the torrent's own tracker.
    let decoded = submitted.body.replace("%3A", ":").replace("%2F", "/");
    assert!(
        decoded.contains(&format!("btih:{content_key}")),
        "{}",
        submitted.body
    );
    assert!(
        decoded.contains("tracker.invalid"),
        "the torrent's trackers travel with it: {}",
        submitted.body
    );
}

/// Put.io offers no selection before it downloads, so the host is never sent to `choose`. Being
/// asked anyway answers with a stable code rather than doing something silent, because "nothing
/// happened" and "your choice was applied" must not look alike.
#[tokio::test]
async fn a_selection_is_refused_with_a_code_and_changes_nothing() {
    let bytes = component();
    let host = MockPutio::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle(), &[1, 2])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "putio_transfers.no_remote_selection");
    assert!(!refusal.retryable);
    assert!(host.requests().is_empty(), "nothing was changed at Put.io");
}

/// `discard` reaches the provider exactly when it is called, and nothing else calls it. It
/// cancels the transfer and deliberately deletes no files: discarding a job rDownloader created
/// is one thing, deleting somebody's files is another.
#[tokio::test]
async fn discard_cancels_the_transfer_only_when_called_and_deletes_no_files() {
    let bytes = component();
    let host = MockPutio::new(&[]);
    let runners = runners_for(Arc::clone(&host), &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("discarded");
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, format!("{API}/transfers/cancel"));
    assert_eq!(requests[0].body, format!("transfer_ids={TRANSFER_ID}"));
    assert!(
        requests
            .iter()
            .all(|request| !request.path.ends_with("/files/delete")),
        "nothing in the account is deleted"
    );
}

/// Nothing that could be a credential, an account's real identifier or a provider's own
/// sentence about somebody's data is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    fn walk(value: &serde_json::Value, path: &std::path::Path) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, inner) in fields {
                    assert!(
                        !matches!(
                            name.as_str(),
                            "access_token" | "oauth_token" | "refresh_token" | "client_secret"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    if name == "hash"
                        && let Some(hash) = inner.as_str()
                    {
                        assert!(
                            hash.eq_ignore_ascii_case(HASH)
                                || hash.chars().all(|character| character == 'f'),
                            "{path:?} carries a real hash"
                        );
                    }
                    // Put.io's `error_message` is written for a developer and is the one field
                    // this plugin deliberately never repeats. The fixtures hold a placeholder
                    // so a test asserting it did not travel is asserting something.
                    if name == "error_message" || name == "status_message" {
                        assert_eq!(
                            inner.as_str(),
                            Some("redacted provider sentence"),
                            "{path:?} carries a real provider sentence"
                        );
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
                    text.contains("invalid") || text.starts_with("https://api.put.io/"),
                    "{path:?} carries a live link: {text}"
                );
            }
            _ => {}
        }
    }
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/putio_transfers");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 16, "only {checked} fixtures were checked");
}

/// The plugin reaches only the API it needs, with only the token it needs, and claims the
/// provider row its two siblings already share.
#[test]
fn the_plugin_reaches_only_the_part_of_put_io_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["putio".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["api.put.io".to_owned()].as_slice()
    );
    // The access token and nothing else: submitting a magnet has no business with the
    // application the person registered.
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["putio_access_token".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}
