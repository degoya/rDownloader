//! The Offcloud cloud-download contract, driven end to end against a mock of the provider
//! (RD-120-02).
//!
//! `plugins/offcloud-cloud/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-offcloud-cloud`
//! -- and the mock stands in for `offcloud.com`. It answers at the host boundary, so no socket
//! is opened, no account is needed and no request leaves the machine; and because it sees each
//! request exactly as the plugin described it, a test can assert that the account's key left
//! the plugin as the template `{{secret:offcloud_api_key}}` and never as a value.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `cloud_created.json` | a handle carrying the request id |
//! | An address is submitted | `cloud_created.json` | the same, keyed in the other key space |
//! | Offcloud has not started yet | `status_created.json` | `Preparing`, with a wait |
//! | Offcloud is fetching | `status_downloading.json` | `Working`, 425 permille |
//! | The job finished | `status_downloaded.json` + `explore_detailed.json` | `Ready`, three addresses with names and folders |
//! | The job finished with one file | `status_downloaded_single.json` + `explore_empty.json` | `Ready`, the one address `status` named |
//! | A finished job read the simple way | `explore_simple.json` | `Ready`, names recovered from the addresses |
//! | The file tree could not be read | a 503 on `cloud/explore` | a wait, never a one-file package |
//! | Offcloud ended it | `status_error.json`, `status_canceled.json` | a refusal that ends the job |
//! | The key is gone | `error_noauth.json` | a refusal that ends the job, `auth_invalid` |
//! | The request budget is spent | `error_too_many_requests.json` + `Retry-After` | a wait carrying the header |
//! | An add-on is missing | `not_available_cloud.json` | a refusal naming the add-on |
//! | A submit was lost in flight | `history.json` | adopted by its content key; a stranger's is not |
//! | Nothing asks for a selection | -- | `choose` is refused rather than silently ignored |
//!
//! **A run against the real provider is not claimed here.** It needs an Offcloud account;
//! `docs/roadmap/jobs/120-02-offcloud.md` records that as open.

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

const MANIFEST: &str = include_str!("../../../plugins/offcloud-cloud/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-000000000139";

// The sanitised fixtures. Every identifier in them is a placeholder and every address points at
// a redacted path; `fixtures_carry_no_credential_material` is what keeps it that way.
const CREATED: &str = include_str!("fixtures/offcloud_cloud/cloud_created.json");
const HISTORY: &str = include_str!("fixtures/offcloud_cloud/history.json");
const STATUS_CREATED: &str = include_str!("fixtures/offcloud_cloud/status_created.json");
const STATUS_DOWNLOADING: &str = include_str!("fixtures/offcloud_cloud/status_downloading.json");
const STATUS_DOWNLOADED: &str = include_str!("fixtures/offcloud_cloud/status_downloaded.json");
const STATUS_SINGLE: &str = include_str!("fixtures/offcloud_cloud/status_downloaded_single.json");
const STATUS_ERROR: &str = include_str!("fixtures/offcloud_cloud/status_error.json");
const STATUS_CANCELED: &str = include_str!("fixtures/offcloud_cloud/status_canceled.json");
const EXPLORE_DETAILED: &str = include_str!("fixtures/offcloud_cloud/explore_detailed.json");
const EXPLORE_SIMPLE: &str = include_str!("fixtures/offcloud_cloud/explore_simple.json");
const EXPLORE_EMPTY: &str = include_str!("fixtures/offcloud_cloud/explore_empty.json");
const ERROR_NOAUTH: &str = include_str!("fixtures/offcloud_cloud/error_noauth.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/offcloud_cloud/error_too_many_requests.json");
const NOT_AVAILABLE: &str = include_str!("fixtures/offcloud_cloud/not_available_cloud.json");

/// The info hash every magnet fixture carries: SHA-1 of nothing, recognisable as a placeholder.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const REQUEST_ID: &str = "REDACTEDREQUEST01";
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
/// The address the third history entry was started from.
const ADDRESS: &str = "https://example.invalid/f/abc123";
const API: &str = "/api";
const KEY_TEMPLATE: &str = "Bearer {{secret:offcloud_api_key}}";

/// The plugin component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-offcloud-cloud")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// One request the plugin made, flattened to what a test asserts on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    content_type: Option<String>,
    body: String,
}

/// The mock Offcloud `cloud/*` API, routed by method and path.
struct MockOffcloud {
    /// What `cloud/status` answers next, in order; the last one repeats.
    status: Mutex<VecDeque<&'static str>>,
    /// What `cloud/explore/{id}` answers, and with which status.
    explore: Mutex<(u16, &'static str)>,
    /// When set, every request is answered with this status, body and `Retry-After`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockOffcloud {
    fn new(status: &[&'static str]) -> Arc<Self> {
        Self::with_explore(status, 200, EXPLORE_DETAILED)
    }

    fn with_explore(status: &[&'static str], code: u16, explore: &'static str) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(status.iter().copied().collect()),
            explore: Mutex::new((code, explore)),
            failure: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, retry_after: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(VecDeque::new()),
            explore: Mutex::new((200, EXPLORE_EMPTY)),
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

    fn next_status(&self) -> &'static str {
        let mut queue = self.status.lock().expect("status");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(STATUS_CREATED)
        }
    }
}

#[async_trait]
impl ResolverHost for MockOffcloud {
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
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            path: path.clone(),
            content_type: request
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                .map(|header| header.value_template.clone()),
            body: String::from_utf8_lossy(&request.body).into_owned(),
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
        let explore = format!("{API}/cloud/explore/{REQUEST_ID}");
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{API}/cloud") => answer(200, CREATED, None),
            ("POST", p) if p == format!("{API}/cloud/status") => {
                answer(200, self.next_status(), None)
            }
            ("GET", p) if p == format!("{API}/cloud/history") => answer(200, HISTORY, None),
            ("GET", p) if p == explore => {
                let (code, body) = *self.explore.lock().expect("explore");
                answer(code, body, None)
            }
            ("POST", p) if p == format!("{API}/cloud/remove") => answer(200, "{}", None),
            _ => answer(404, r#"{"error":"not_found"}"#, None),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "offcloud_api_key"
    }
}

fn runners(host: Arc<MockOffcloud>, bytes: &[u8]) -> RemoteJobRunners {
    let plugin = RemoteJobPlugin::new(manifest(), bytes, Some(host)).expect("the plugin builds");
    RemoteJobRunners::from_plugins(vec![plugin])
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

fn handle() -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: REQUEST_ID.to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: None,
    }
}

fn magnet_key() -> String {
    format!("btih:{HASH}")
}

#[tokio::test]
async fn the_plugin_compiles_against_the_remote_job_world() {
    let bytes = component();
    RemoteJobPlugin::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
}

/// RD-130-11: no cache query is bound for Offcloud, so the plugin names no cache kind and
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

/// The two calls that reach nothing: whether the source is one of ours at all, and the key it
/// is known by. Both spellings of one info hash are one key, which is what makes a magnet
/// copied from two sites one job and not two.
#[tokio::test]
async fn both_shapes_are_claimed_and_keyed_in_two_key_spaces() {
    let bytes = component();
    let host = MockOffcloud::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);

    assert_eq!(
        runners.identify("offcloud", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: magnet_key(),
        }
    );
    assert_eq!(
        runners
            .identify(
                "offcloud",
                &RemoteJobSource::Magnet(MAGNET_BASE32.to_owned())
            )
            .await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: magnet_key(),
        }
    );

    // The third shape of source (RD-120-20), and the one that makes Offcloud different from the
    // Real-Debrid remote job: `POST /api/cloud` takes an ordinary address as readily as a
    // magnet, so an address is claimed here rather than refused.
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("offcloud", &RemoteJobSource::Address(ADDRESS.to_owned()))
        .await
    else {
        panic!("an address is claimed");
    };
    assert!(content_key.starts_with("url:"), "{content_key}");
    assert_eq!(content_key.len(), "url:".len() + 40);
    // The two key spaces are one unique index, and they must not be able to collide in it.
    assert_ne!(content_key, magnet_key());

    // Bytes have nowhere to go: `POST /api/cloud` takes one field and it is an address.
    let container = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31eee".to_vec();
    assert_eq!(
        runners
            .identify("offcloud", &RemoteJobSource::Container(container))
            .await,
        StartOutcome::NotClaimed
    );
    // A magnet naming something other than a torrent, and an address nothing could fetch.
    for foreign in [
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        "magnet:?dn=Example.Release",
    ] {
        assert_eq!(
            runners
                .identify("offcloud", &RemoteJobSource::Magnet(foreign.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{foreign}"
        );
    }
    assert!(
        host.requests().is_empty(),
        "claiming and identifying reach nothing"
    );
}

/// The key of one source is the same every time it is asked, which is the property the whole
/// duplicate guard rests on: the host writes it into `remote_jobs(account_id, content_key)`
/// before anything is handed over, and a key that drifted would defeat the index it is for.
#[tokio::test]
async fn the_content_key_is_stable_across_calls_and_reaches_no_provider() {
    let bytes = component();
    let host = MockOffcloud::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    for source in [
        magnet(),
        RemoteJobSource::Magnet(MAGNET_BASE32.to_owned()),
        RemoteJobSource::Address(ADDRESS.to_owned()),
    ] {
        let first = runners.identify("offcloud", &source).await;
        let second = runners.identify("offcloud", &source).await;
        assert_eq!(first, second, "{source:?}");
    }
    assert!(host.requests().is_empty());
}

/// The whole way through, in the order the sweep drives it: submitted, waited for, fetched,
/// and finished as addresses with names and the folders they sat in.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_that_keep_their_folders() {
    let bytes = component();
    let host = MockOffcloud::new(&[STATUS_CREATED, STATUS_DOWNLOADING, STATUS_DOWNLOADED]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Submit: one request, a form body carrying the magnet, and the id the provider named.
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, REQUEST_ID);
    assert_eq!(handle.account_id, account.to_string());
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/cloud"));
    assert_eq!(
        submitted.content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert!(
        submitted
            .body
            .starts_with("url=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{}",
        submitted.body
    );

    // Created: nobody is needed, and the plugin suggests a wait the host will clamp.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(10)
        }
    );

    // Downloading: Offcloud's own byte counts, in thousandths.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(425));

    // Finished: three files, each with its name and the folder it sat in inside the job.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 3);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[0].size, Some(10));
    assert_eq!(
        artifacts[0].package_hint.as_deref(),
        Some("Example.Release")
    );
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ep02.mkv"));
    // The nested folder survives, and the job's name is not repeated inside itself.
    assert_eq!(artifacts[2].file_name.as_deref(), Some("en.srt"));
    assert_eq!(
        artifacts[2].package_hint.as_deref(),
        Some("Example.Release/Subs")
    );

    // A finished job costs two requests: the status, then the tree. `status` knows at most one
    // address and no paths at all, so a multi-file job read from it alone would arrive as one
    // nameless entry.
    let last = host.requests();
    assert_eq!(last[last.len() - 2].path, format!("{API}/cloud/status"));
    assert_eq!(
        last[last.len() - 1].path,
        format!("{API}/cloud/explore/{REQUEST_ID}")
    );

    // Nothing in the chain removed anything.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.ends_with("/cloud/remove")),
        "discard is never a side effect"
    );
}

/// An address is submitted exactly as a magnet is, in the field Offcloud has for both.
#[tokio::test]
async fn an_address_is_submitted_through_the_same_field_as_a_magnet() {
    let bytes = component();
    let host = MockOffcloud::new(&[STATUS_CREATED]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("offcloud", &RemoteJobSource::Address(ADDRESS.to_owned()))
        .await
    else {
        panic!("an address is claimed");
    };
    let handle = runners
        .submit(
            PLUGIN_ID,
            account,
            &RemoteJobSource::Address(ADDRESS.to_owned()),
            &content_key,
        )
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, REQUEST_ID);
    assert_eq!(
        host.requests()[0].body,
        "url=https%3A%2F%2Fexample.invalid%2Ff%2Fabc123"
    );
}

/// A job with exactly one file: nothing to explore, and the address `status` named is the
/// answer rather than an empty package.
#[tokio::test]
async fn a_one_file_job_falls_back_to_the_address_the_status_named() {
    let bytes = component();
    let host = MockOffcloud::with_explore(&[STATUS_SINGLE], 200, EXPLORE_EMPTY);
    let runners = runners(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected the address");
    };
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("single.bin"));
    // A single file does not need a folder of its own, and inventing one would put every
    // one-file job in a package by itself.
    assert_eq!(artifacts[0].package_hint, None);
}

/// An outage on the *second* of a finished job's two calls is a wait, not a smaller job.
///
/// The failure mode this guards against is silent: `cloud/status` names one address and
/// `cloud/explore` names all of them, so swallowing a refusal from the second would close a
/// finished multi-file job with one file in it — permanently, and with nothing to say the
/// others ever existed. A refusal worth waiting out is carried out instead, and the host polls
/// again.
#[tokio::test]
async fn an_outage_while_reading_the_file_tree_is_a_wait_and_not_a_one_file_package() {
    let bytes = component();
    let host = MockOffcloud::with_explore(&[STATUS_DOWNLOADED], 503, "");
    let runners = runners(Arc::clone(&host), &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected a wait rather than a truncated package");
    };
    assert!(refusal.retryable);
    assert_eq!(refusal.code, "offcloud_cloud.server_error");
    // Both calls were made; the job is simply not finished being read.
    let paths: Vec<String> = host.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        vec![
            format!("{API}/cloud/status"),
            format!("{API}/cloud/explore/{REQUEST_ID}"),
        ]
    );
}

/// The other shape `explore` is described in: addresses and nothing else. The names are
/// recovered from the addresses rather than left empty.
#[tokio::test]
async fn a_tree_of_bare_addresses_still_produces_named_files() {
    let bytes = component();
    let host = MockOffcloud::with_explore(&[STATUS_DOWNLOADED], 200, EXPLORE_SIMPLE);
    let runners = runners(Arc::clone(&host), &bytes);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ep02.mkv"));
}

/// The key the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_api_key_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockOffcloud::new(&[STATUS_CREATED]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect("submitted");
    let _ = runners.poll(PLUGIN_ID, account, &handle).await;
    let _ = runners.adopt(PLUGIN_ID, account, &magnet_key()).await;
    let _ = runners.discard(PLUGIN_ID, account, &handle).await;
    let authorizations = host.authorizations();
    assert_eq!(authorizations.len(), 4);
    assert!(
        authorizations.iter().all(|value| value == KEY_TEMPLATE),
        "{authorizations:?}"
    );
    // The published API also offers `?key=<api key>`. It is deliberately not used: a query
    // parameter ends up in every redirect chain and every log line that quotes an address.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.contains("key=")),
        "the key never travels in an address"
    );
}

/// A job the provider ended is a refusal that ends it here too, under the code that says how.
#[tokio::test]
async fn a_job_the_provider_ended_is_a_failure_and_not_a_wait() {
    let bytes = component();
    for (fixture, code) in [
        (STATUS_ERROR, "offcloud_cloud.job_failed"),
        (STATUS_CANCELED, "offcloud_cloud.job_canceled"),
    ] {
        let host = MockOffcloud::new(&[fixture]);
        let runners = runners(host, &bytes);
        let PollOutcome::Refused(refusal) =
            runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
        else {
            panic!("expected a refusal for {code}");
        };
        assert_eq!(refusal.code, code);
        assert!(!refusal.retryable, "{code}");
    }
}

/// `NOAUTH`: the key is gone. Waiting does not bring it back, so the job ends and the person is
/// told to enter a key again rather than watching a job poll for ever.
#[tokio::test]
async fn an_expired_key_ends_the_job_under_its_own_code() {
    let bytes = component();
    // Offcloud answers this with a 200 and a document, which is exactly why the document is
    // read before the status.
    let host = MockOffcloud::failing(200, ERROR_NOAUTH, None);
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "offcloud_cloud.auth_invalid");
    assert!(!refusal.retryable);
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "offcloud_cloud.auth_invalid");
    assert!(!refusal.retryable);
}

/// A 429 with `Retry-After`: the account's request budget is spent. A wait, with the provider's
/// own figure carried out for the host to clamp -- and the figure survives the prose the answer
/// carries beside it, which is the ordering this fixture exists for.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let bytes = component();
    let host = MockOffcloud::failing(429, ERROR_TOO_MANY, Some("120"));
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "offcloud_cloud.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
}

/// An add-on the account has not bought: not the source's fault, so the job is not marked dead
/// under a code about the link.
#[tokio::test]
async fn a_missing_addon_is_refused_under_its_own_code_and_names_the_addon() {
    let bytes = component();
    let host = MockOffcloud::failing(200, NOT_AVAILABLE, None);
    let runners = runners(host, &bytes);
    let refusal = runners
        .submit(PLUGIN_ID, AccountId::new(), &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "offcloud_cloud.addon_required");
}

/// The crash window, closed at the provider: a submit whose answer never arrived is found again
/// in the account's own history, and somebody else's job is not.
///
/// The comparison is the point. Offcloud's history records the *address* each job was started
/// from, and the fixture records the same content in the other spelling than the one submitted
/// -- base32 where the caller pasted hex. Comparing the strings would miss it; putting the
/// recorded link through the derivation `identify` used is what finds it.
#[tokio::test]
async fn a_lost_submit_is_adopted_by_its_content_key_and_a_stranger_is_not() {
    let bytes = component();
    let host = MockOffcloud::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let adopted = runners
        .adopt(PLUGIN_ID, account, &magnet_key())
        .await
        .expect("listed")
        .expect("found by its content key");
    assert_eq!(adopted.remote_id, REQUEST_ID);
    let listed = &host.requests()[0];
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, format!("{API}/cloud/history"));

    // An address job in the same history is found by its own key, in the other key space.
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("offcloud", &RemoteJobSource::Address(ADDRESS.to_owned()))
        .await
    else {
        panic!("an address is claimed");
    };
    let adopted = runners
        .adopt(PLUGIN_ID, account, &content_key)
        .await
        .expect("listed")
        .expect("found by its content key");
    assert_eq!(adopted.remote_id, "REDACTEDREQUEST03");

    // Somebody else's job is not adopted, whatever else is in the account.
    assert_eq!(
        runners
            .adopt(
                PLUGIN_ID,
                account,
                "btih:ffffffffffffffffffffffffffffffffffffffff"
            )
            .await
            .expect("listed"),
        None
    );
}

/// Restart and idempotency, in the two properties the row depends on.
///
/// The persistent half lives in `rd-api`'s sweep and is exercised there against a mock
/// provider. What has to hold *of this plugin* is what that sweep assumes: the key is derived
/// without a request, so the duplicate guard can fire before one; and after a crash the same
/// key finds the job the provider already holds, so the second attempt adopts instead of
/// creating a second cloud download in somebody's account.
#[tokio::test]
async fn a_restart_finds_the_job_it_already_started_rather_than_starting_a_second() {
    let bytes = component();
    let host = MockOffcloud::new(&[STATUS_DOWNLOADING]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Before the crash: the key is written down, then the submit goes out.
    let StartOutcome::Identified { content_key, .. } =
        runners.identify("offcloud", &magnet()).await
    else {
        panic!("the magnet is claimed");
    };
    let before = runners
        .submit(PLUGIN_ID, account, &magnet(), &content_key)
        .await
        .expect("submitted");
    let submits = host
        .requests()
        .iter()
        .filter(|request| request.path == format!("{API}/cloud"))
        .count();
    assert_eq!(submits, 1);

    // After the crash the source is all that is left, and it keys to the same value -- the
    // plugin remembers nothing of its own between calls, which is the whole reason this has to
    // be derived rather than stored.
    let StartOutcome::Identified {
        content_key: again, ..
    } = runners.identify("offcloud", &magnet()).await
    else {
        panic!("the magnet is claimed");
    };
    assert_eq!(again, content_key);

    // The adoption check finds the very job the lost submit created, so nothing is submitted a
    // second time.
    let adopted = runners
        .adopt(PLUGIN_ID, account, &again)
        .await
        .expect("listed")
        .expect("found by its content key");
    assert_eq!(adopted.remote_id, before.remote_id);
    let submits = host
        .requests()
        .iter()
        .filter(|request| request.path == format!("{API}/cloud"))
        .count();
    assert_eq!(submits, 1, "the adoption path submits nothing");

    // And the adopted handle polls as the original would have.
    assert!(matches!(
        runners.poll(PLUGIN_ID, account, &adopted).await,
        PollOutcome::Working(_)
    ));
}

/// Nothing at Offcloud ever waits for a selection, so `choose` refuses rather than letting one
/// be made and quietly ignored.
#[tokio::test]
async fn a_selection_is_refused_because_the_provider_has_no_such_step() {
    let bytes = component();
    let host = MockOffcloud::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle(), &[1, 2])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "offcloud_cloud.no_selection");
    assert!(!refusal.retryable);

    // An empty one never reaches the guest at all: the host refuses it first.
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle(), &[])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "remote_job.empty_choice");
    assert!(host.requests().is_empty());
}

/// Cancelling and removing at the provider are two different acts, and only the second reaches
/// Offcloud.
///
/// Everything above polls, submits, adopts and finishes jobs without a single `cloud/remove`
/// crossing the boundary; this is the other half, that the one call which does remove reaches
/// exactly the endpoint it should when it is explicitly made. What rDownloader did not put in
/// somebody's account on its own, it does not take out on its own.
#[tokio::test]
async fn removing_at_the_provider_reaches_offcloud_only_when_it_is_asked_for() {
    let bytes = component();
    let host = MockOffcloud::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("discarded");
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, format!("{API}/cloud/remove"));
    assert_eq!(requests[0].body, r#"{"requestIds":["REDACTEDREQUEST01"]}"#);
    assert_eq!(
        requests[0].content_type.as_deref(),
        Some("application/json"),
        "the one call whose parameter is a list"
    );
}

/// Nothing that could be a credential, an account's real identifier or a live address is
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
                            "apiKey" | "api_key" | "key" | "token" | "password" | "connect.sid"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    if matches!(name.as_str(), "requestId" | "userId")
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
                    text.contains("REDACTED") || text.contains("example.invalid"),
                    "{path:?} carries a live address: {text}"
                );
            }
            // A magnet in a fixture names the SHA-1 of nothing, or a run of zeroes: both are
            // recognisable as placeholders and neither is somebody's content.
            serde_json::Value::String(text) if text.starts_with("magnet:") => {
                let lower = text.to_ascii_lowercase();
                assert!(
                    lower.contains(HASH)
                        || lower.contains("3i42h3s6nnfq2msvx7xzkyaysc")
                        || lower.contains(&"0".repeat(40)),
                    "{path:?} carries a real info hash: {text}"
                );
            }
            _ => {}
        }
    }
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/offcloud_cloud");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} fixtures were checked");
}

/// The plugin reaches only the API it needs, with only the credential it needs, and claims the
/// provider row its resolver sibling carries.
#[test]
fn the_plugin_reaches_only_the_part_of_offcloud_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["offcloud".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["offcloud.com".to_owned()].as_slice()
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["offcloud_api_key".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}

/// The resolver sibling carries the `[provider]` row this plugin claims, and the two agree on
/// the credential slot.
///
/// A manifest carries exactly one `plugin_type`, so the two halves of Offcloud are two
/// packages -- and if they ever disagreed on the slug or the secret reference, the remote-job
/// plugin would have an account nobody can create and a key it cannot read.
#[test]
fn the_two_offcloud_packages_agree_on_the_provider_and_the_credential() {
    let resolver: PluginManifest =
        toml::from_str(include_str!("../../../plugins/offcloud/manifest.toml"))
            .expect("the resolver manifest");
    let provider = resolver.provider.as_ref().expect("a provider section");
    assert_eq!(provider.slug, "offcloud");
    let extension = manifest().extension.as_ref().expect("an extension").clone();
    assert_eq!(extension.claims, vec![provider.slug.clone()]);
    assert_eq!(
        resolver.capabilities.secrets,
        manifest().capabilities.secrets,
        "one account, one credential slot"
    );
}
