//! The Seedr transfer contract, driven end to end against a mock of the provider (RD-120-04).
//!
//! `plugins/seedr-jobs/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-seedr-jobs` --
//! and the mock stands in for `www.seedr.cc`. It answers at the host boundary, so no socket is
//! opened, no account is needed and no request leaves the machine; and because it sees each
//! request exactly as the plugin described it, a test can assert that the account's password
//! left the plugin inside the template `{{basic:seedr_password}}` and that neither half of the
//! credential is anywhere in what was sent.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `transfer_created.json` | a handle carrying the transfer id and its name |
//! | A `.torrent` is submitted | the same | the same job, keyed on the same info hash |
//! | Seedr has not started yet | `folder_root_fresh.json` | `Preparing`, with a wait |
//! | Seedr is fetching | `folder_root_running.json` | `Working`, 425 permille |
//! | The transfer became a folder | `folder_root_done.json` + `folder_release.json` + `folder_subs.json` | `Ready`, three addresses keeping their folders |
//! | The folder is empty | `folder_release_empty.json` | a refusal, never an empty package |
//! | The transfer is simply gone | `folder_root_empty.json` | a refusal that ends the job |
//! | The credential was refused | `error_auth.json` | a refusal that ends the job, `auth_invalid` |
//! | The request budget is spent | `error_rate_limit.json` + `Retry-After` | a wait carrying the header |
//! | The plan does not include the API | `error_plan.json` | a refusal naming the plan |
//! | The account is full | `error_not_enough_space.json` | a wait, because Seedr kept the content |
//! | A submit was lost in flight | `folder_root_adopt.json` | adopted by its content key; a stranger's is not |
//! | Nothing asks for a selection | -- | `choose` is refused rather than silently ignored |
//! | Removing reaches Seedr only when asked | `transfer_removed.json` | one `DELETE`, and never a side effect |
//!
//! **A run against the real provider is not claimed here.** It needs a premium Seedr account --
//! the API is a paid feature by Seedr's own documentation -- and
//! `docs/roadmap/jobs/120-04-seedr-feasibility.md` records that as open.

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

const MANIFEST: &str = include_str!("../../../plugins/seedr-jobs/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-000000000181";

// The sanitised fixtures. Every identifier in them is a small placeholder integer and every
// info hash is the SHA-1 of nothing or a run of `f`; `fixtures_carry_no_credential_material` is
// what keeps it that way.
const CREATED: &str = include_str!("fixtures/seedr_jobs/transfer_created.json");
const ROOT_FRESH: &str = include_str!("fixtures/seedr_jobs/folder_root_fresh.json");
const ROOT_RUNNING: &str = include_str!("fixtures/seedr_jobs/folder_root_running.json");
const ROOT_DONE: &str = include_str!("fixtures/seedr_jobs/folder_root_done.json");
const ROOT_EMPTY: &str = include_str!("fixtures/seedr_jobs/folder_root_empty.json");
const ROOT_ADOPT: &str = include_str!("fixtures/seedr_jobs/folder_root_adopt.json");
const RELEASE: &str = include_str!("fixtures/seedr_jobs/folder_release.json");
const RELEASE_EMPTY: &str = include_str!("fixtures/seedr_jobs/folder_release_empty.json");
const SUBS: &str = include_str!("fixtures/seedr_jobs/folder_subs.json");
const ERROR_AUTH: &str = include_str!("fixtures/seedr_jobs/error_auth.json");
const ERROR_RATE_LIMIT: &str = include_str!("fixtures/seedr_jobs/error_rate_limit.json");
const ERROR_PLAN: &str = include_str!("fixtures/seedr_jobs/error_plan.json");
const ERROR_SPACE: &str = include_str!("fixtures/seedr_jobs/error_not_enough_space.json");
const REMOVED: &str = include_str!("fixtures/seedr_jobs/transfer_removed.json");

/// The info hash every fixture carries: SHA-1 of nothing, recognisable as a placeholder.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const TRANSFER_ID: &str = "11";
const JOB_NAME: &str = "Example.Release";
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
const REST: &str = "/rest";
const AUTHORIZATION_TEMPLATE: &str = "Basic {{basic:seedr_password}}";

/// The plugin component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-seedr-jobs")
}

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// One minimal bencoded torrent naming the same twenty bytes the magnet above does is not
/// possible -- the info hash of a container is whatever its own `info` dictionary hashes to --
/// so the container fixture is built here and its key is read from the plugin rather than
/// asserted against a constant.
fn container() -> Vec<u8> {
    b"d8:announce31:http://tracker.invalid/announce4:infod4:name15:Example.Release6:lengthi31eee"
        .to_vec()
}

/// One request the plugin made, flattened to what a test asserts on.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    content_type: Option<String>,
    body: String,
}

/// The mock Seedr REST API, routed by method and path.
struct MockSeedr {
    /// What `GET /rest/folder` answers next, in order; the last one repeats.
    root: Mutex<VecDeque<&'static str>>,
    /// What `GET /rest/folder/5` answers.
    release: &'static str,
    /// When set, every request is answered with this status, body and `Retry-After`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockSeedr {
    fn new(root: &[&'static str]) -> Arc<Self> {
        Self::with_release(root, RELEASE)
    }

    fn with_release(root: &[&'static str], release: &'static str) -> Arc<Self> {
        Arc::new(Self {
            root: Mutex::new(root.iter().copied().collect()),
            release,
            failure: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, retry_after: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            root: Mutex::new(VecDeque::new()),
            release: RELEASE,
            failure: Some((status, body, retry_after)),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|one| one.path).collect()
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }

    fn next_root(&self) -> &'static str {
        let mut queue = self.root.lock().expect("root");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(ROOT_EMPTY)
        }
    }
}

#[async_trait]
impl ResolverHost for MockSeedr {
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
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{REST}/transfer/magnet") => answer(200, CREATED, None),
            ("GET", p) if p == format!("{REST}/folder") => answer(200, self.next_root(), None),
            ("GET", p) if p == format!("{REST}/folder/5") => answer(200, self.release, None),
            ("GET", p) if p == format!("{REST}/folder/6") => answer(200, SUBS, None),
            ("DELETE", p) if p == format!("{REST}/transfer/{TRANSFER_ID}") => {
                answer(200, REMOVED, None)
            }
            _ => answer(404, r#"{"result":false,"error":"not_found"}"#, None),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "seedr_password"
    }
}

fn runners(host: Arc<MockSeedr>, bytes: &[u8]) -> RemoteJobRunners {
    let plugin = RemoteJobPlugin::new(manifest(), bytes, Some(host)).expect("the plugin builds");
    RemoteJobRunners::from_plugins(vec![plugin])
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

/// The handle the host hands back after a submit, including the name it remembered.
fn handle() -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: TRANSFER_ID.to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: Some(JOB_NAME.to_owned()),
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

/// RD-130-11: no cache query is bound for Seedr, so the plugin names no cache kind and
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

/// The two calls that reach nothing: whether the source is one of ours at all, and the key it is
/// known by. Both spellings of one info hash are one key, which is what makes a magnet copied
/// from two sites one job and not two.
#[tokio::test]
async fn both_torrent_shapes_are_claimed_and_keyed_on_the_info_hash() {
    let bytes = component();
    let host = MockSeedr::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);

    for spelling in [MAGNET, MAGNET_BASE32] {
        assert_eq!(
            runners
                .identify("seedr", &RemoteJobSource::Magnet(spelling.to_owned()))
                .await,
            StartOutcome::Identified {
                plugin_id: PLUGIN_ID.to_owned(),
                content_key: magnet_key(),
            },
            "{spelling}"
        );
    }

    // A `.torrent` is claimed too: its bytes are re-expressed as the magnet they are equivalent
    // to, so Seedr's one documented body shape covers both.
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("seedr", &RemoteJobSource::Container(container()))
        .await
    else {
        panic!("a container is claimed");
    };
    assert!(content_key.starts_with("btih:"), "{content_key}");
    assert_eq!(content_key.len(), "btih:".len() + 40);

    // An ordinary web address is deliberately not claimed: Seedr's `transfer/url` is a torrent
    // source, and claiming every http address for it would take them away from the hosters that
    // can actually fetch them.
    assert_eq!(
        runners
            .identify(
                "seedr",
                &RemoteJobSource::Address("https://example.invalid/f/abc123".to_owned())
            )
            .await,
        StartOutcome::NotClaimed
    );
    // A magnet naming something other than a torrent is not ours either.
    for foreign in [
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        "magnet:?dn=Example.Release",
    ] {
        assert_eq!(
            runners
                .identify("seedr", &RemoteJobSource::Magnet(foreign.to_owned()))
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
    let host = MockSeedr::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    for source in [
        magnet(),
        RemoteJobSource::Magnet(MAGNET_BASE32.to_owned()),
        RemoteJobSource::Container(container()),
    ] {
        let first = runners.identify("seedr", &source).await;
        let second = runners.identify("seedr", &source).await;
        assert_eq!(first, second, "{source:?}");
    }
    assert!(host.requests().is_empty());
}

/// The whole way through, in the order the sweep drives it: submitted, waited for, fetched, and
/// finished as addresses that keep the folders they sat in.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_that_keep_their_folders() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_FRESH, ROOT_RUNNING, ROOT_DONE]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Submit: one request, a form body carrying the magnet, and the transfer Seedr named.
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TRANSFER_ID);
    assert_eq!(handle.account_id, account.to_string());
    // The name is on the handle, because it is what a finished transfer's folder is found by
    // and a guest remembers nothing between calls.
    assert_eq!(handle.job_state.as_deref(), Some(JOB_NAME));
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{REST}/transfer/magnet"));
    assert_eq!(
        submitted.content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert!(
        submitted
            .body
            .starts_with("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{}",
        submitted.body
    );

    // Fresh: Seedr has the transfer and has not started, so nobody is shown a bar at zero.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(15)
        }
    );

    // Running: Seedr's own per cent, in thousandths.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(425));

    // Finished: the transfer is gone from the list and the folder it became is there instead.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 3);
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(
        artifacts[0].url.as_str(),
        "https://www.seedr.cc/rest/file/101"
    );
    assert_eq!(artifacts[0].size, Some(10));
    assert_eq!(artifacts[0].package_hint.as_deref(), Some(JOB_NAME));
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ep02.mkv"));
    // The nested folder survives, and the transfer's name is not repeated inside itself.
    assert_eq!(artifacts[2].file_name.as_deref(), Some("en.srt"));
    assert_eq!(
        artifacts[2].package_hint.as_deref(),
        Some("Example.Release/Subs")
    );

    // A finished transfer costs the listing that found it plus one per folder walked.
    let paths = host.paths();
    assert_eq!(
        &paths[paths.len() - 3..],
        [
            format!("{REST}/folder"),
            format!("{REST}/folder/5"),
            format!("{REST}/folder/6"),
        ]
    );
    // And the transfer endpoint named after polling is never asked, because a finished transfer
    // could only ever answer it with "not here", which is also what a deleted one answers.
    assert!(
        !paths
            .iter()
            .any(|path| path.starts_with(&format!("{REST}/transfer/{TRANSFER_ID}"))),
        "{paths:?}"
    );
    // Nothing in the chain removed anything.
    assert!(
        host.requests()
            .iter()
            .all(|request| request.method != "DELETE"),
        "discard is never a side effect"
    );
}

/// A `.torrent` reaches Seedr through the one body shape it documents for a magnet, carrying the
/// same twenty bytes -- which is what keeps a magnet and the matching file one job.
#[tokio::test]
async fn a_container_is_submitted_as_the_magnet_it_is_equivalent_to() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_RUNNING]);
    let runners = runners(Arc::clone(&host), &bytes);
    let source = RemoteJobSource::Container(container());
    let StartOutcome::Identified { content_key, .. } = runners.identify("seedr", &source).await
    else {
        panic!("a container is claimed");
    };
    let handle = runners
        .submit(PLUGIN_ID, AccountId::new(), &source, &content_key)
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TRANSFER_ID);
    let body = &host.requests()[0].body;
    assert!(
        body.starts_with("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{body}"
    );
    // The torrent's own name and its tracker travel with it, which is what lets Seedr start a
    // transfer whose peers are not reachable over DHT alone.
    assert!(body.contains("dn%3DExample.Release"), "{body}");
    assert!(body.contains("tr%3Dhttp"), "{body}");
    assert_eq!(
        host.requests()[0].path,
        format!("{REST}/transfer/magnet"),
        "the multipart upload endpoint is deliberately never reached"
    );
}

/// A transfer that is gone and left no folder behind is what a transfer somebody removed in
/// Seedr's own interface looks like. Waiting for it would poll an account for ever.
#[tokio::test]
async fn a_transfer_that_vanished_without_a_folder_ends_the_job() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_EMPTY]);
    let runners = runners(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "seedr_jobs.transfer_gone");
    assert!(!refusal.retryable);
}

/// Finished with nothing is not a result: a package with no files in it and nothing to explain
/// why is the shape of the defect ADR 0001 was opened for.
#[tokio::test]
async fn a_finished_transfer_whose_folder_is_empty_is_a_refusal_and_not_a_package() {
    let bytes = component();
    let host = MockSeedr::with_release(&[ROOT_DONE], RELEASE_EMPTY);
    let runners = runners(host, &bytes);
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "seedr_jobs.no_files");
    assert!(!refusal.retryable);
}

/// The credential the plugin sends is the vault template, on every request, and neither half of
/// it is ever a value.
#[tokio::test]
async fn the_credential_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_RUNNING]);
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
        authorizations
            .iter()
            .all(|value| value == AUTHORIZATION_TEMPLATE),
        "{authorizations:?}"
    );
    // Not in an address either. Seedr's credential is an e-mail address and a password, and a
    // query parameter ends up in every redirect chain and every log line that quotes a URL.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.contains('@') && !request.path.contains("password")),
        "the credential never travels in an address"
    );
}

/// A credential Seedr refused: waiting does not make it right, so the job ends and the person is
/// told to enter it again rather than watching a job poll for ever.
#[tokio::test]
async fn a_refused_credential_ends_the_job_under_its_own_code() {
    let bytes = component();
    let host = MockSeedr::failing(401, ERROR_AUTH, None);
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "seedr_jobs.auth_invalid");
    assert!(!refusal.retryable);
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "seedr_jobs.auth_invalid");
    assert!(!refusal.retryable);
}

/// A 429 with `Retry-After`: the account's request budget is spent. A wait, with the provider's
/// own figure carried out for the host to clamp -- and the figure survives the prose the answer
/// carries beside it, which is the ordering this fixture exists for.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let bytes = component();
    let host = MockSeedr::failing(429, ERROR_RATE_LIMIT, Some("120"));
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "seedr_jobs.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
}

/// Seedr's own documentation makes the REST API a premium feature, so a plan that does not
/// include it is something a person can act on -- and must not be filed under "some 4xx".
#[tokio::test]
async fn a_plan_that_does_not_include_the_api_is_refused_under_its_own_code() {
    let bytes = component();
    let host = MockSeedr::failing(402, ERROR_PLAN, None);
    let runners = runners(host, &bytes);
    let refusal = runners
        .submit(PLUGIN_ID, AccountId::new(), &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "seedr_jobs.plan_required");
    assert!(!refusal.retryable);
}

/// A full account waits rather than failing: Seedr's own answer says in its name that it kept
/// the content, so a person who frees space has a job that can still run. It arrives with a
/// 200, which is why the document is read before the status.
#[tokio::test]
async fn a_full_account_waits_because_seedr_kept_the_content() {
    let bytes = component();
    let host = MockSeedr::failing(200, ERROR_SPACE, None);
    let runners = runners(host, &bytes);
    let refusal = runners
        .submit(PLUGIN_ID, AccountId::new(), &magnet(), &magnet_key())
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "seedr_jobs.out_of_space");
    assert!(refusal.retryable);
}

/// The crash window, closed at the provider: a submit whose answer never arrived is found again
/// among the account's running transfers, and somebody else's is not.
///
/// The comparison is on the twenty bytes rather than on a name. Seedr records the info hash of
/// every running transfer, and the fixture spells it in upper case where the caller pasted it in
/// lower -- comparing the strings would miss it.
#[tokio::test]
async fn a_lost_submit_is_adopted_by_its_content_key_and_a_stranger_is_not() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_ADOPT]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let adopted = runners
        .adopt(PLUGIN_ID, account, &magnet_key())
        .await
        .expect("listed")
        .expect("found by its content key");
    assert_eq!(adopted.remote_id, TRANSFER_ID);
    // The name comes back with it, so the poll that follows can still find the folder the
    // transfer becomes.
    assert_eq!(adopted.job_state.as_deref(), Some(JOB_NAME));
    let listed = &host.requests()[0];
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, format!("{REST}/folder"));

    // Somebody else's transfer is not adopted, whatever else is in the account.
    assert_eq!(
        runners
            .adopt(
                PLUGIN_ID,
                account,
                "btih:0000000000000000000000000000000000000000"
            )
            .await
            .expect("listed"),
        None
    );
}

/// Restart and idempotency, in the two properties the row depends on.
///
/// The persistent half lives in `rd-api`'s sweep and is exercised there against a mock provider.
/// What has to hold *of this plugin* is what that sweep assumes: the key is derived without a
/// request, so the duplicate guard can fire before one; and after a crash the same key finds the
/// transfer the provider already holds, so the second attempt adopts instead of creating a
/// second transfer in somebody's account.
#[tokio::test]
async fn a_restart_finds_the_transfer_it_already_started_rather_than_starting_a_second() {
    let bytes = component();
    let host = MockSeedr::new(&[ROOT_ADOPT]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Before the crash: the key is written down, then the submit goes out.
    let StartOutcome::Identified { content_key, .. } = runners.identify("seedr", &magnet()).await
    else {
        panic!("the magnet is claimed");
    };
    let before = runners
        .submit(PLUGIN_ID, account, &magnet(), &content_key)
        .await
        .expect("submitted");
    let submits = |host: &MockSeedr| {
        host.paths()
            .iter()
            .filter(|path| *path == &format!("{REST}/transfer/magnet"))
            .count()
    };
    assert_eq!(submits(&host), 1);

    // After the crash the source is all that is left, and it keys to the same value -- the
    // plugin remembers nothing of its own between calls, which is the whole reason this has to
    // be derived rather than stored.
    let StartOutcome::Identified {
        content_key: again, ..
    } = runners.identify("seedr", &magnet()).await
    else {
        panic!("the magnet is claimed");
    };
    assert_eq!(again, content_key);

    // The adoption check finds the very transfer the lost submit created, so nothing is
    // submitted a second time.
    let adopted = runners
        .adopt(PLUGIN_ID, account, &again)
        .await
        .expect("listed")
        .expect("found by its content key");
    assert_eq!(adopted.remote_id, before.remote_id);
    assert_eq!(submits(&host), 1, "the adoption path submits nothing");

    // And the adopted handle polls as the original would have.
    assert!(matches!(
        runners.poll(PLUGIN_ID, account, &adopted).await,
        PollOutcome::Working(_)
    ));
}

/// Nothing at Seedr ever waits for a selection, so `choose` refuses rather than letting one be
/// made and quietly ignored.
///
/// This is the fifth provider of this world to answer the same way, and it is a limit of the
/// contract rather than of the plugin: `choose` returns no handle and nothing the host stores
/// about the answer reaches the guest again, so a guest could not tell an answered question from
/// an unasked one (RD-120-35). Expressing a selection by deleting the unwanted files at Seedr
/// was considered and rejected -- that is exactly the implicit remote deletion ADR 0003 forbids.
#[tokio::test]
async fn a_selection_is_refused_because_the_provider_has_no_such_step() {
    let bytes = component();
    let host = MockSeedr::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle(), &[1, 2])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "seedr_jobs.no_selection");
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
/// Seedr.
///
/// Everything above polls, submits, adopts and finishes transfers without a single `DELETE`
/// crossing the boundary; this is the other half, that the one call which does remove reaches
/// exactly the endpoint it should when it is explicitly made. What rDownloader did not put in
/// somebody's account on its own, it does not take out on its own -- and note that what is
/// removed is the *transfer*. A finished transfer's folder is the person's files, and the
/// confirmed request said to discard the job.
#[tokio::test]
async fn removing_at_the_provider_reaches_seedr_only_when_it_is_asked_for() {
    let bytes = component();
    let host = MockSeedr::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("discarded");
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].path, format!("{REST}/transfer/{TRANSFER_ID}"));
    assert!(requests[0].body.is_empty());
}

/// A transfer Seedr no longer holds is nothing left to remove, so a confirmed discard of one
/// succeeds instead of failing with a refusal nobody can act on.
#[tokio::test]
async fn discarding_a_transfer_that_is_already_gone_succeeds() {
    let bytes = component();
    let host = MockSeedr::failing(404, r#"{"result":false,"error":"not_found"}"#, None);
    let runners = runners(host, &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("already gone is discarded");
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
                            "api_key" | "key" | "token" | "password" | "email" | "username"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    if name == "torrent_hash"
                        && let Some(hash) = inner.as_str()
                    {
                        let lower = hash.to_ascii_lowercase();
                        assert!(
                            lower == HASH || lower.chars().all(|one| one == 'f' || one == '0'),
                            "{path:?} carries a real info hash: {hash}"
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
                    text.contains("example.invalid"),
                    "{path:?} carries a live address: {text}"
                );
            }
            _ => {}
        }
    }
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seedr_jobs");
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
fn the_plugin_reaches_only_the_part_of_seedr_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["seedr".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["www.seedr.cc".to_owned()].as_slice()
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["seedr_password".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}

/// The resolver sibling carries the `[provider]` row this plugin claims, and the two agree on
/// the credential slot.
///
/// A manifest carries exactly one `plugin_type`, so the two halves of Seedr are two packages --
/// and if they ever disagreed on the slug or the secret reference, the remote-job plugin would
/// have an account nobody can create and a credential it cannot read.
#[test]
fn the_two_seedr_packages_agree_on_the_provider_and_the_credential() {
    let resolver: PluginManifest =
        toml::from_str(include_str!("../../../plugins/seedr/manifest.toml"))
            .expect("the resolver manifest");
    let provider = resolver.provider.as_ref().expect("a provider section");
    assert_eq!(provider.slug, "seedr");
    // Both halves of a HTTP Basic credential have to exist for either plugin to build a
    // request, which is why the provider row requires a user name at all.
    assert!(provider.username_required);
    let extension = manifest().extension.as_ref().expect("an extension").clone();
    assert_eq!(extension.claims, vec![provider.slug.clone()]);
    assert_eq!(
        resolver.capabilities.secrets,
        manifest().capabilities.secrets,
        "one account, one credential slot"
    );
}
