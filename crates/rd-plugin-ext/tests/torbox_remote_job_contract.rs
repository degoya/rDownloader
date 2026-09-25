//! The TorBox remote-job contract, driven end to end against a mock of the provider
//! (RD-120-01).
//!
//! `plugins/torbox-jobs/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-torbox-jobs`
//! -- and the mock stands in for `api.torbox.app`. It answers at the host boundary, so no
//! socket is opened, no account is needed and no request leaves the machine; and because it
//! sees each request exactly as the plugin described it, a test can assert that the account's
//! key left the plugin as the template `{{secret:torbox_api_key}}` and never as a value.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `create_torrent.json` | a handle carrying the job's id |
//! | An NZB is submitted | `create_usenet.json` | the Usenet endpoints, not the torrent ones |
//! | A web link is submitted | `create_webdl.json` | the web-download endpoints |
//! | The job is parked | `mylist_queued.json` | `Preparing`, with a wait |
//! | The job is running | `mylist_downloading.json` | `Working`, 425 permille |
//! | The job finished | `mylist_completed.json` | `Ready`, two addresses with names and a folder |
//! | TorBox served it from cache | `mylist_cached.json` | `Ready` by the same rule, no promise |
//! | TorBox ended it | `mylist_error.json`, `mylist_missing_files.json` | a refusal that ends the job |
//! | The key expired | `error_bad_token.json` | a refusal that ends the job, `auth_invalid` |
//! | The request budget is spent | `error_too_many_requests.json` + `Retry-After` | a wait carrying the header |
//! | A submit was lost in flight | `mylist_adopt.json` | adopted by its digest; a stranger's is not |
//! | The cache is asked (RD-130-11) | `checkcached_torrents.json`, `checkcached_usenet.json`, `checkcached_webdl.json` | one `GET` per kind, `cached` with name and size, never `known` |
//! | The cache answers as a list | `checkcached_list.json` | read alike, `cached` |
//! | The cache holds nothing | `checkcached_none.json` (`"data": null`) | `unknown` |
//! | The key expired during a cache check | `error_bad_token.json` | the whole call refused, `auth_invalid` |
//!
//! **A run against the real provider is not claimed here.** It needs a TorBox account with an
//! API key; `docs/roadmap/jobs/120-01-torbox.md` records that as open.

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
    CacheAnswer, CacheKind, CacheQuery, CacheState, PollOutcome, RemoteJobRunners, StartOutcome,
};
use rd_plugin_host::{
    PluginManifest, PluginType,
    extension::{RemoteJobHandle, RemoteJobPlugin, RemoteJobSource},
};

const MANIFEST: &str = include_str!("../../../plugins/torbox-jobs/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-00000000011e";

// The sanitised fixtures. Every identifier in them is a placeholder and no answer carries an
// account's own value; `fixtures_carry_no_credential_material` is what keeps it that way.
const CREATE_TORRENT: &str = include_str!("fixtures/torbox_jobs/create_torrent.json");
const CREATE_USENET: &str = include_str!("fixtures/torbox_jobs/create_usenet.json");
const CREATE_WEBDL: &str = include_str!("fixtures/torbox_jobs/create_webdl.json");
const MYLIST_QUEUED: &str = include_str!("fixtures/torbox_jobs/mylist_queued.json");
const MYLIST_DOWNLOADING: &str = include_str!("fixtures/torbox_jobs/mylist_downloading.json");
const MYLIST_COMPLETED: &str = include_str!("fixtures/torbox_jobs/mylist_completed.json");
const MYLIST_CACHED: &str = include_str!("fixtures/torbox_jobs/mylist_cached.json");
const MYLIST_ERROR: &str = include_str!("fixtures/torbox_jobs/mylist_error.json");
const MYLIST_MISSING: &str = include_str!("fixtures/torbox_jobs/mylist_missing_files.json");
const MYLIST_ADOPT: &str = include_str!("fixtures/torbox_jobs/mylist_adopt.json");
const MYLIST_EMPTY: &str = include_str!("fixtures/torbox_jobs/mylist_empty.json");
const ERROR_BAD_TOKEN: &str = include_str!("fixtures/torbox_jobs/error_bad_token.json");
const ERROR_TOO_MANY: &str = include_str!("fixtures/torbox_jobs/error_too_many_requests.json");
const CHECKCACHED_TORRENTS: &str = include_str!("fixtures/torbox_jobs/checkcached_torrents.json");
const CHECKCACHED_USENET: &str = include_str!("fixtures/torbox_jobs/checkcached_usenet.json");
const CHECKCACHED_WEBDL: &str = include_str!("fixtures/torbox_jobs/checkcached_webdl.json");
const CHECKCACHED_LIST: &str = include_str!("fixtures/torbox_jobs/checkcached_list.json");
const CHECKCACHED_NONE: &str = include_str!("fixtures/torbox_jobs/checkcached_none.json");

/// The info hash every torrent fixture carries: SHA-1 of nothing, recognisable as a
/// placeholder.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const TORRENT_KEY: &str = "torrent:da39a3ee5e6b4b0d3255bfef95601890afd80709";
const JOB_ID: &str = "4711";
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
const WEB_ADDRESS: &str = "https://example.invalid/Example.Release.bin";
const API: &str = "/v1/api";
const KEY_TEMPLATE: &str = "Bearer {{secret:torbox_api_key}}";

/// A minimal single-file torrent, and the NZB beside it. Both are read locally and never sent
/// anywhere but the mock.
fn torrent_container() -> Vec<u8> {
    b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee".to_vec()
}

fn nzb_container() -> Vec<u8> {
    br#"<?xml version="1.0" encoding="iso-8859-1" ?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="nobody@example.invalid" date="1" subject="Example.Release (1/1)">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="10" number="1">part1@example</segment></segments>
  </file>
</nzb>
"#
    .to_vec()
}

/// The plugin component, or a failure naming the build command when it has not been built in
/// this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-torbox-jobs")
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
    body: String,
}

/// The mock TorBox API, routed by method and path.
struct MockTorBox {
    /// What a `mylist` for one job answers next, in order; the last one repeats.
    info: Mutex<VecDeque<&'static str>>,
    /// The digest the adoption list reports for the job that is ours.
    hash: Mutex<String>,
    /// When set, every request is answered with this status, body and `Retry-After`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    /// The digests TorBox's caches hold (RD-130-11); every other one is not held.
    held: Mutex<Vec<String>>,
    /// When set, the fixture every `checkcached` hit is answered with, instead of the one
    /// keyed by hash that belongs to the endpoint.
    cache_form: Mutex<Option<&'static str>>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockTorBox {
    fn new(info: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            info: Mutex::new(info.iter().copied().collect()),
            hash: Mutex::new(HASH.to_owned()),
            failure: None,
            held: Mutex::new(Vec::new()),
            cache_form: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, retry_after: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            info: Mutex::new(VecDeque::new()),
            hash: Mutex::new(HASH.to_owned()),
            failure: Some((status, body, retry_after)),
            held: Mutex::new(Vec::new()),
            cache_form: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    /// Sets the digest the account's own job is listed under, for the two kinds whose digest
    /// is derived from the bytes rather than known in advance.
    fn holding(self: &Arc<Self>, digest: &str) {
        *self.hash.lock().expect("hash") = digest.to_owned();
    }

    /// Makes TorBox's caches hold these digests.
    fn holds(self: &Arc<Self>, digests: &[&str]) {
        *self.held.lock().expect("held") =
            digests.iter().map(|digest| (*digest).to_owned()).collect();
    }

    /// Answers every `checkcached` hit with this fixture.
    fn answers_cache_with(self: &Arc<Self>, fixture: &'static str) {
        *self.cache_form.lock().expect("cache form") = Some(fixture);
    }

    /// A `checkcached` answer: the endpoint's fixture carrying the first asked digest that is
    /// held, or the one that holds nothing.
    fn cache_answer(&self, path: &str, query: &[(String, String)]) -> String {
        let held = self.held.lock().expect("held");
        let hit = query
            .iter()
            .filter(|(name, _)| name == "hash")
            .flat_map(|(_, value)| value.split(','))
            .find(|asked| held.iter().any(|digest| digest.eq_ignore_ascii_case(asked)));
        let Some(hit) = hit else {
            return CHECKCACHED_NONE.to_owned();
        };
        let fixture =
            self.cache_form
                .lock()
                .expect("cache form")
                .unwrap_or(match path.rsplit('/').nth(1) {
                    Some("torrents") => CHECKCACHED_TORRENTS,
                    Some("usenet") => CHECKCACHED_USENET,
                    _ => CHECKCACHED_WEBDL,
                });
        fixture.replace("{{hash}}", hit)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("authorizations").clone()
    }

    fn next_info(&self) -> &'static str {
        let mut queue = self.info.lock().expect("info");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(MYLIST_QUEUED)
        }
    }

    /// The adoption list, with the account's own job under the digest under test.
    fn adoption_list(&self) -> String {
        MYLIST_ADOPT.replace("{{hash}}", &self.hash.lock().expect("hash"))
    }
}

#[async_trait]
impl ResolverHost for MockTorBox {
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
        let named = query.iter().any(|(name, _)| name == "id");
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{API}/torrents/createtorrent") => {
                answer(200, CREATE_TORRENT, None)
            }
            ("POST", p) if p == format!("{API}/usenet/createusenetdownload") => answer(
                200,
                &CREATE_USENET.replace("{{hash}}", &self.hash.lock().expect("hash")),
                None,
            ),
            ("POST", p) if p == format!("{API}/webdl/createwebdownload") => answer(
                200,
                &CREATE_WEBDL.replace("{{hash}}", &self.hash.lock().expect("hash")),
                None,
            ),
            // One list endpoint answers two questions: one job by its id, or the page the
            // adoption check reads.
            ("GET", p)
                if p == format!("{API}/torrents/mylist")
                    || p == format!("{API}/usenet/mylist")
                    || p == format!("{API}/webdl/mylist") =>
            {
                if named {
                    answer(200, self.next_info(), None)
                } else {
                    answer(200, &self.adoption_list(), None)
                }
            }
            ("GET", p)
                if p == format!("{API}/torrents/checkcached")
                    || p == format!("{API}/usenet/checkcached")
                    || p == format!("{API}/webdl/checkcached") =>
            {
                answer(200, &self.cache_answer(p, &query), None)
            }
            ("POST", p)
                if p == format!("{API}/torrents/controltorrent")
                    || p == format!("{API}/usenet/controlusenetdownload")
                    || p == format!("{API}/webdl/controlwebdownload") =>
            {
                answer(200, r#"{"success":true,"error":null,"data":null}"#, None)
            }
            _ => answer(
                404,
                r#"{"success":false,"error":"ENDPOINT_NOT_FOUND","detail":"no such endpoint"}"#,
                None,
            ),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "torbox_api_key"
    }
}

fn runners(host: Arc<MockTorBox>, bytes: &[u8]) -> RemoteJobRunners {
    let plugin = RemoteJobPlugin::new(manifest(), bytes, Some(host)).expect("the plugin builds");
    RemoteJobRunners::from_plugins(vec![plugin])
}

fn runners_for(host: &Arc<MockTorBox>, bytes: &[u8]) -> RemoteJobRunners {
    runners(Arc::clone(host), bytes)
}

fn md5_hex(text: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn query(source: RemoteJobSource, kind: CacheKind) -> CacheQuery {
    CacheQuery { source, kind }
}

/// An indexer's NZB link. The `apikey` is a placeholder: a real one would be hashed by the
/// plugin and never sent, which is what the host's check-before-hash comment is about.
const NZB_LINK: &str = "https://indexer.invalid/api?t=get&id=REDACTED01&apikey=REDACTED";

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

fn handle(job_state: &str) -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: JOB_ID.to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: Some(job_state.to_owned()),
    }
}

fn identified(outcome: StartOutcome) -> String {
    match outcome {
        StartOutcome::Identified { content_key, .. } => content_key,
        other => panic!("expected an identified source, got {other:?}"),
    }
}

#[tokio::test]
async fn the_plugin_compiles_against_the_remote_job_world() {
    let bytes = component();
    RemoteJobPlugin::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
}

/// The two calls that reach nothing. All three shapes of `job-source` are claimed, each is
/// keyed by the digest TorBox knows it by, and the kind travels in front of that digest so an
/// NZB's key can never be mistaken for a web link's on one account.
#[tokio::test]
async fn all_three_sources_are_claimed_and_the_key_carries_the_kind() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);

    assert_eq!(
        runners.identify("torbox", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: TORRENT_KEY.to_owned(),
        }
    );
    // Both spellings of one info hash are one key, which is what makes a magnet copied from
    // two sites one job and not two.
    assert_eq!(
        identified(
            runners
                .identify("torbox", &RemoteJobSource::Magnet(MAGNET_BASE32.to_owned()))
                .await
        ),
        TORRENT_KEY
    );
    let torrent = identified(
        runners
            .identify("torbox", &RemoteJobSource::Container(torrent_container()))
            .await,
    );
    assert!(torrent.starts_with("torrent:"), "{torrent}");
    assert_ne!(
        torrent, TORRENT_KEY,
        "a different torrent is a different key"
    );
    let usenet = identified(
        runners
            .identify("torbox", &RemoteJobSource::Container(nzb_container()))
            .await,
    );
    assert!(usenet.starts_with("usenet:"), "{usenet}");
    let web = identified(
        runners
            .identify("torbox", &RemoteJobSource::Address(WEB_ADDRESS.to_owned()))
            .await,
    );
    assert!(web.starts_with("web:"), "{web}");
    // Derived, never remembered: a fresh instantiation answers the same way, which is the half
    // of the duplicate guarantee that belongs to the plugin.
    assert_eq!(
        identified(
            runners
                .identify("torbox", &RemoteJobSource::Container(nzb_container()))
                .await
        ),
        usenet
    );
    for foreign in [
        "https://example.invalid/not-a-magnet",
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
    ] {
        assert_eq!(
            runners
                .identify("torbox", &RemoteJobSource::Magnet(foreign.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{foreign}"
        );
    }
    // An address the host would refuse anyway is refused here too, rather than relied on.
    for refused in ["file:///etc/passwd", "ftp://example.invalid/x"] {
        assert_eq!(
            runners
                .identify("torbox", &RemoteJobSource::Address(refused.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{refused}"
        );
    }
    assert!(
        host.requests().is_empty(),
        "claiming and identifying reach nothing"
    );
}

/// The whole way through, in the order the sweep drives it: submitted, parked, running, and
/// finished as addresses with names and a folder.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_the_link_grabber_can_take() {
    let bytes = component();
    let host = MockTorBox::new(&[MYLIST_QUEUED, MYLIST_DOWNLOADING, MYLIST_COMPLETED]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Submit: one request, a multipart body carrying the magnet, and the id TorBox named.
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), TORRENT_KEY)
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, JOB_ID);
    assert_eq!(handle.account_id, account.to_string());
    // The kind travels on the handle, so a poll knows which of three endpoints to ask.
    assert_eq!(handle.job_state.as_deref(), Some("torrent"));
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/torrents/createtorrent"));
    assert!(
        submitted.body.contains("name=\"magnet\""),
        "{}",
        submitted.body
    );
    assert!(submitted.body.contains(MAGNET), "{}", submitted.body);

    // Parked: nobody is needed, and the plugin suggests a wait the host will clamp.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(30)
        }
    );
    // The list is asked for one job, and never out of TorBox's own cache: a cached list is
    // exactly the one that cannot show the job created a second ago.
    let polled = host.requests().last().cloned().expect("the poll");
    assert_eq!(polled.path, format!("{API}/torrents/mylist"));
    assert!(polled.query.contains(&("id".to_owned(), JOB_ID.to_owned())));
    assert!(
        polled
            .query
            .contains(&("bypass_cache".to_owned(), "true".to_owned()))
    );

    // Running: TorBox's fraction, in the thousandths the contract carries.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(425));
    assert_eq!(work.speed_bytes_per_second, Some(1_048_576));
    assert_eq!(work.seconds_remaining, Some(120));

    // Finished: both files, each with its name, its folder and an address carrying no key.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 2);
    assert_eq!(
        artifacts[0].url.as_str(),
        "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=4711&file_id=0"
    );
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[0].size, Some(10));
    assert_eq!(
        artifacts[0].package_hint.as_deref(),
        Some("Example.Release")
    );
    assert_eq!(artifacts[1].file_name.as_deref(), Some("sample.mkv"));
    assert_eq!(
        artifacts[1].package_hint.as_deref(),
        Some("Example.Release/Sample")
    );
    // The address the queue keeps is the stable one. The key is added by the resolver sibling
    // at fetch time, which is what makes it renewable rather than a value dead in a row.
    for artifact in &artifacts {
        assert!(!artifact.url.as_str().contains("token"), "{}", artifact.url);
    }
    // Nothing in the chain deleted anything.
    assert!(
        host.requests()
            .iter()
            .all(|request| !request.path.contains("control")),
        "discard is never a side effect"
    );
}

/// The three kinds reach three sets of endpoints, and the one state machine drives all of
/// them: an NZB and a web link finish exactly as a magnet does.
#[tokio::test]
async fn an_nzb_and_a_web_link_run_on_their_own_endpoints() {
    let bytes = component();
    for (source, create, list, id_field) in [
        (
            RemoteJobSource::Container(nzb_container()),
            format!("{API}/usenet/createusenetdownload"),
            format!("{API}/usenet/mylist"),
            "usenet_id",
        ),
        (
            RemoteJobSource::Address(WEB_ADDRESS.to_owned()),
            format!("{API}/webdl/createwebdownload"),
            format!("{API}/webdl/mylist"),
            "web_id",
        ),
    ] {
        let host = MockTorBox::new(&[MYLIST_COMPLETED]);
        let runners = runners(Arc::clone(&host), &bytes);
        let account = AccountId::new();
        let key = identified(runners.identify("torbox", &source).await);
        let handle = runners
            .submit(PLUGIN_ID, account, &source, &key)
            .await
            .expect("submitted");
        let submitted = &host.requests()[0];
        assert_eq!(submitted.path, create, "{key}");
        // A container goes in as a file part; an address goes in as a text field.
        if matches!(source, RemoteJobSource::Address(_)) {
            assert!(
                submitted.body.contains("name=\"link\""),
                "{}",
                submitted.body
            );
            assert!(submitted.body.contains(WEB_ADDRESS), "{}", submitted.body);
        } else {
            assert!(
                submitted.body.contains("filename=\"upload.nzb\""),
                "{}",
                submitted.body
            );
            assert!(submitted.body.contains("<nzb"), "{}", submitted.body);
        }
        let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
            panic!("expected the addresses for {key}");
        };
        assert_eq!(host.requests().last().expect("the poll").path, list);
        assert!(
            artifacts[0].url.as_str().contains(&format!("{id_field}=")),
            "{}",
            artifacts[0].url
        );
    }
}

/// A job TorBox served out of its own cache reaches `Ready` by the same rule as one it
/// fetched: the cache is a fact about how fast it went, not a state of its own, and nothing
/// here presents it as a promise.
#[tokio::test]
async fn a_cache_hit_is_ready_by_the_same_rule_as_anything_else() {
    let bytes = component();
    let host = MockTorBox::new(&[MYLIST_CACHED]);
    let runners = runners(host, &bytes);
    let PollOutcome::Ready(artifacts) = runners
        .poll(PLUGIN_ID, AccountId::new(), &handle("torrent"))
        .await
    else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 1);
}

/// The key the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_api_key_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockTorBox::new(&[MYLIST_COMPLETED]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), TORRENT_KEY)
        .await
        .expect("submitted");
    let _ = runners.poll(PLUGIN_ID, account, &handle).await;
    let _ = runners.adopt(PLUGIN_ID, account, TORRENT_KEY).await;
    let authorizations = host.authorizations();
    assert_eq!(authorizations.len(), 3);
    assert!(
        authorizations.iter().all(|value| value == KEY_TEMPLATE),
        "{authorizations:?}"
    );
}

/// A job TorBox ended is a refusal that ends it, under the code that says how.
#[tokio::test]
async fn a_job_the_provider_ended_is_a_failure_and_not_a_wait() {
    let bytes = component();
    for (fixture, code) in [
        (MYLIST_ERROR, "torbox_jobs.job_failed"),
        (MYLIST_MISSING, "torbox_jobs.job_incomplete"),
    ] {
        let host = MockTorBox::new(&[fixture]);
        let runners = runners(host, &bytes);
        let PollOutcome::Refused(refusal) = runners
            .poll(PLUGIN_ID, AccountId::new(), &handle("torrent"))
            .await
        else {
            panic!("expected a refusal for {code}");
        };
        assert_eq!(refusal.code, code);
        assert!(!refusal.retryable, "{code}");
    }
}

/// A job that is not in the account any more ends rather than being polled for ever.
#[tokio::test]
async fn a_job_the_account_no_longer_holds_ends_the_row() {
    let bytes = component();
    let host = MockTorBox::new(&[MYLIST_EMPTY]);
    let runners = runners(host, &bytes);
    let PollOutcome::Refused(refusal) = runners
        .poll(PLUGIN_ID, AccountId::new(), &handle("torrent"))
        .await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "torbox_jobs.job_gone");
}

/// `BAD_TOKEN` inside a `200`: the key is gone. Waiting does not bring it back, so the job
/// ends and the person is told to paste a key again rather than watching a job poll for ever.
#[tokio::test]
async fn an_expired_key_ends_the_job_under_its_own_code() {
    let bytes = component();
    let host = MockTorBox::failing(200, ERROR_BAD_TOKEN, None);
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle("torrent")).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "torbox_jobs.auth_invalid");
    assert!(!refusal.retryable);
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), TORRENT_KEY)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "torbox_jobs.auth_invalid");
    assert!(!refusal.retryable);
}

/// `TOO_MANY_REQUESTS` with `Retry-After`: the account's request budget is spent. A wait, with
/// TorBox's own figure carried out for the host to clamp -- refused requests count towards the
/// cap that refused them, so asking again at once would only extend it.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let bytes = component();
    let host = MockTorBox::failing(429, ERROR_TOO_MANY, Some("120"));
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle("torrent")).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "torbox_jobs.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), TORRENT_KEY)
        .await
        .expect_err("refused");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
}

/// The crash window, closed at the provider: a submit whose answer never arrived is found
/// again by its digest in the account's own list, and somebody else's job is not.
///
/// All three kinds, because the digest is derived three different ways and an adoption that
/// worked for torrents alone would leave the other two submitting twice.
#[tokio::test]
async fn an_orphaned_job_is_adopted_by_its_digest_and_a_stranger_is_not() {
    let bytes = component();
    for (source, list) in [
        (magnet(), format!("{API}/torrents/mylist")),
        (
            RemoteJobSource::Container(nzb_container()),
            format!("{API}/usenet/mylist"),
        ),
        (
            RemoteJobSource::Address(WEB_ADDRESS.to_owned()),
            format!("{API}/webdl/mylist"),
        ),
    ] {
        let host = MockTorBox::new(&[]);
        let runners = runners(Arc::clone(&host), &bytes);
        let account = AccountId::new();
        let key = identified(runners.identify("torbox", &source).await);
        let (_, digest) = key.split_once(':').expect("the kind in front");
        host.holding(digest);

        let adopted = runners
            .adopt(PLUGIN_ID, account, &key)
            .await
            .expect("listed")
            .expect("found by its digest");
        assert_eq!(adopted.remote_id, JOB_ID, "{key}");
        let listed = &host.requests()[0];
        assert_eq!(listed.method, "GET");
        assert_eq!(listed.path, list, "{key}");
        assert!(
            listed
                .query
                .contains(&("limit".to_owned(), "100".to_owned())),
            "one page, never a walk through the whole account: {:?}",
            listed.query
        );
        // Somebody else's job is in the very same list and is not taken.
        let stranger = format!(
            "{}:{}",
            key.split(':').next().expect("a kind"),
            "f".repeat(40)
        );
        assert_eq!(
            runners
                .adopt(PLUGIN_ID, account, &stranger)
                .await
                .expect("listed"),
            None,
            "{stranger}"
        );
    }
}

/// The plugin's half of "a restart creates no duplicate": the key is derived from the source
/// alone, so a process that stopped between writing the row and hearing back derives exactly
/// the same key afterwards -- and the adoption that key drives finds the job rather than
/// creating a second one.
///
/// The other half is the unique index on `(account_id, content_key)`, which
/// `crates/rd-db/tests/remote_jobs.rs` drives.
#[tokio::test]
async fn a_restart_derives_the_same_key_and_adopts_instead_of_submitting_again() {
    let bytes = component();
    let source = RemoteJobSource::Container(nzb_container());
    let account = AccountId::new();

    // Before the crash: the key is derived and the job is created.
    let before = MockTorBox::new(&[]);
    let first = runners(Arc::clone(&before), &bytes);
    let key = identified(first.identify("torbox", &source).await);
    let created = first
        .submit(PLUGIN_ID, account, &source, &key)
        .await
        .expect("submitted");
    assert_eq!(created.remote_id, JOB_ID);
    let created_key = key.clone();

    // After the restart: a brand-new adapter, a brand-new guest, nothing remembered.
    let after = MockTorBox::new(&[]);
    after.holding(key.split_once(':').expect("a kind").1);
    let second = runners(Arc::clone(&after), &bytes);
    assert_eq!(
        identified(second.identify("torbox", &source).await),
        created_key,
        "the key is derived from the source and from nothing else"
    );
    let adopted = second
        .adopt(PLUGIN_ID, account, &created_key)
        .await
        .expect("listed")
        .expect("the job created before the restart");
    assert_eq!(adopted.remote_id, created.remote_id);
    assert_eq!(adopted.job_state, created.job_state);
    // Nothing was created a second time.
    assert!(
        after
            .requests()
            .iter()
            .all(|request| !request.path.contains("create")),
        "{:?}",
        after.requests()
    );
}

/// TorBox fetches whole jobs and offers no call that says "these three files and not the
/// others", so the plugin refuses rather than reporting a selection nothing acted on. The
/// person picks in the LinkGrabber the finished addresses land in.
#[tokio::test]
async fn torbox_offers_no_file_selection_and_says_so() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle("torrent"), &[1, 2])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "torbox_jobs.no_selection");
    assert!(host.requests().is_empty(), "nothing was asked of TorBox");
    // An empty choice never reaches the guest at all: the host refuses one first.
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle("torrent"), &[])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "remote_job.empty_choice");
}

/// `discard` reaches the provider exactly when it is called, with the field that endpoint
/// spells the job by, and nothing else calls it.
#[tokio::test]
async fn discard_reaches_the_provider_only_when_called() {
    let bytes = component();
    for (kind, path, field) in [
        (
            "torrent",
            format!("{API}/torrents/controltorrent"),
            "torrent_id",
        ),
        (
            "usenet",
            format!("{API}/usenet/controlusenetdownload"),
            "usenet_id",
        ),
        ("web", format!("{API}/webdl/controlwebdownload"), "webdl_id"),
    ] {
        let host = MockTorBox::new(&[]);
        let runners = runners(Arc::clone(&host), &bytes);
        runners
            .discard(PLUGIN_ID, AccountId::new(), &handle(kind))
            .await
            .expect("discarded");
        let requests = host.requests();
        assert_eq!(requests.len(), 1, "{kind}");
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, path);
        let body: serde_json::Value = serde_json::from_str(&requests[0].body).expect("a JSON body");
        assert_eq!(body[field], JOB_ID, "{kind}");
        assert_eq!(body["operation"], "delete", "{kind}");
    }
}

/// RD-130-11: TorBox can be asked about all three of its caches, and says so once per load.
#[tokio::test]
async fn all_three_caches_can_be_asked() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let plugin =
        RemoteJobPlugin::new(manifest(), &bytes, Some(Arc::clone(&host) as _)).expect("builds");
    assert_eq!(
        plugin.cache_kinds().await.expect("kinds"),
        vec![CacheKind::Torrent, CacheKind::Usenet, CacheKind::Hoster]
    );
    let runners = runners(Arc::clone(&host), &bytes);
    assert_eq!(
        runners.cache_providers().await,
        vec![(
            "torbox".to_owned(),
            vec![CacheKind::Torrent, CacheKind::Usenet, CacheKind::Hoster]
        )]
    );
    assert!(
        host.requests().is_empty(),
        "naming the kinds reaches nothing"
    );
}

/// RD-130-11: a mixed batch costs exactly one `GET` per kind, each answer lands at its
/// query's position, a held digest is `cached` with TorBox's name and size, and one TorBox
/// does not hold is `unknown` -- never `known`, because TorBox names only what it holds.
///
/// Hex and base32 spell one info hash and hit one entry; an NZB link and a hoster link are
/// asked by the MD5 of the address, at the Usenet and the web-download cache respectively.
#[tokio::test]
async fn a_mixed_batch_asks_each_cache_once_and_answers_in_place() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let nzb_digest = md5_hex(NZB_LINK);
    let web_digest = md5_hex(WEB_ADDRESS);
    host.holds(&[HASH, nzb_digest.as_str()]);
    let runners = runners(Arc::clone(&host), &bytes);
    let queries = vec![
        query(magnet(), CacheKind::Torrent),
        query(
            RemoteJobSource::Magnet(MAGNET_BASE32.to_owned()),
            CacheKind::Torrent,
        ),
        query(
            RemoteJobSource::Address(NZB_LINK.to_owned()),
            CacheKind::Usenet,
        ),
        query(
            RemoteJobSource::Address(WEB_ADDRESS.to_owned()),
            CacheKind::Hoster,
        ),
        query(
            RemoteJobSource::Container(torrent_container()),
            CacheKind::Torrent,
        ),
        // A magnet is no hoster link: nothing to derive, `unknown` without a request.
        query(magnet(), CacheKind::Hoster),
    ];
    let answers = runners
        .check_cached("torbox", AccountId::new(), &queries)
        .await
        .expect("answered");
    assert_eq!(answers.len(), queries.len());
    let held_torrent = CacheAnswer {
        state: CacheState::Cached,
        file_name: Some("Example.Release".to_owned()),
        size: Some(31),
    };
    assert_eq!(answers[0], held_torrent);
    assert_eq!(answers[1], held_torrent, "base32 is the same twenty bytes");
    assert_eq!(
        answers[2],
        CacheAnswer {
            state: CacheState::Cached,
            file_name: Some("Example.Release.Usenet".to_owned()),
            size: Some(2048),
        }
    );
    assert_eq!(answers[3], CacheAnswer::unknown(), "not held is not known");
    assert_eq!(answers[4], CacheAnswer::unknown());
    assert_eq!(answers[5], CacheAnswer::unknown());
    assert!(
        answers
            .iter()
            .all(|answer| answer.state != CacheState::Known),
        "TorBox never says `known`"
    );

    let requests = host.requests();
    assert_eq!(requests.len(), 3, "one request per kind: {requests:?}");
    for (request, (kind, digests)) in requests.iter().zip([
        ("torrents", vec![HASH.to_owned()]),
        ("usenet", vec![nzb_digest.clone()]),
        ("webdl", vec![web_digest.clone()]),
    ]) {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, format!("{API}/{kind}/checkcached"));
        let hashes: Vec<&str> = request
            .query
            .iter()
            .find(|(name, _)| name == "hash")
            .map(|(_, value)| value.split(',').collect())
            .expect("a hash parameter");
        for digest in &digests {
            assert!(hashes.contains(&digest.as_str()), "{kind}: {hashes:?}");
        }
        assert!(
            request
                .query
                .contains(&("format".to_owned(), "object".to_owned()))
        );
        assert!(
            request
                .query
                .contains(&("list_files".to_owned(), "false".to_owned()))
        );
        // The address itself never leaves the plugin, only its digest.
        assert!(
            !request
                .query
                .iter()
                .any(|(_, value)| value.contains("indexer.invalid")),
            "{kind}"
        );
    }
    // Hex and base32 are one digest, asked once; the container is the second torrent digest.
    let torrent_hashes = requests[0]
        .query
        .iter()
        .find(|(name, _)| name == "hash")
        .map(|(_, value)| value.split(',').count());
    assert_eq!(torrent_hashes, Some(2));
    assert!(
        host.authorizations()
            .iter()
            .all(|template| template == KEY_TEMPLATE),
        "the key travels as a template, never as a value"
    );
    assert_eq!(host.authorizations().len(), 3);
}

/// RD-130-11: nothing that can be derived is nothing to ask -- a batch without a digest makes
/// no request at all.
#[tokio::test]
async fn a_batch_without_a_digest_reaches_nothing() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let answers = runners
        .check_cached(
            "torbox",
            AccountId::new(),
            &[
                query(magnet(), CacheKind::Hoster),
                query(
                    RemoteJobSource::Container(nzb_container()),
                    CacheKind::Torrent,
                ),
                query(
                    RemoteJobSource::Address("https://example.invalid/not-a-magnet".to_owned()),
                    CacheKind::Torrent,
                ),
            ],
        )
        .await
        .expect("answered");
    assert_eq!(answers, vec![CacheAnswer::unknown(); 3]);
    assert!(host.requests().is_empty());
}

/// RD-130-11: the answer's shape is not pinned down by TorBox, so `data` as a list is read
/// like `data` keyed by hash, and `"data": null` holds nothing.
#[tokio::test]
async fn a_cache_answer_as_a_list_or_as_null_is_read_alike() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    host.holds(&[HASH]);
    host.answers_cache_with(CHECKCACHED_LIST);
    let runners = runners(Arc::clone(&host), &bytes);
    let answers = runners
        .check_cached(
            "torbox",
            AccountId::new(),
            &[query(magnet(), CacheKind::Torrent)],
        )
        .await
        .expect("answered");
    assert_eq!(
        answers,
        vec![CacheAnswer {
            state: CacheState::Cached,
            file_name: Some("Example.Release".to_owned()),
            size: Some(31),
        }]
    );

    let empty = MockTorBox::new(&[]);
    let runners = runners_for(&empty, &bytes);
    let answers = runners
        .check_cached(
            "torbox",
            AccountId::new(),
            &[
                query(magnet(), CacheKind::Torrent),
                query(
                    RemoteJobSource::Address(WEB_ADDRESS.to_owned()),
                    CacheKind::Hoster,
                ),
            ],
        )
        .await
        .expect("answered");
    assert_eq!(answers, vec![CacheAnswer::unknown(); 2]);
    assert_eq!(empty.requests().len(), 2);
}

/// RD-130-11: a refusal drops the whole call -- no answer survives it, and the code is the one
/// every other TorBox call reports for an expired key.
#[tokio::test]
async fn an_expired_key_refuses_the_whole_cache_check() {
    let bytes = component();
    let host = MockTorBox::failing(200, ERROR_BAD_TOKEN, None);
    let runners = runners(host, &bytes);
    let refusal = runners
        .check_cached(
            "torbox",
            AccountId::new(),
            &[
                query(magnet(), CacheKind::Torrent),
                query(
                    RemoteJobSource::Address(WEB_ADDRESS.to_owned()),
                    CacheKind::Hoster,
                ),
            ],
        )
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "torbox_jobs.auth_invalid");
    assert!(!refusal.retryable);
}

/// RD-130-11: a hundred queries of each kind -- the most one call carries -- stay inside the
/// manifest's fuel and time budget: three calls, three requests, three hundred answers.
#[tokio::test]
async fn a_hundred_queries_of_each_kind_stay_within_the_budget() {
    let bytes = component();
    let host = MockTorBox::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let mut queries = Vec::new();
    for index in 0..100_u32 {
        queries.push(query(
            RemoteJobSource::Magnet(format!("magnet:?xt=urn:btih:{index:040x}")),
            CacheKind::Torrent,
        ));
    }
    for index in 0..100 {
        queries.push(query(
            RemoteJobSource::Address(format!(
                "https://indexer.invalid/api?t=get&id=REDACTED{index:04}&apikey=REDACTED"
            )),
            CacheKind::Usenet,
        ));
    }
    for index in 0..100 {
        queries.push(query(
            RemoteJobSource::Address(format!(
                "https://example.invalid/REDACTED/{index:04}/Example.Release.bin"
            )),
            CacheKind::Hoster,
        ));
    }
    let held = format!("{:040x}", 7);
    host.holds(&[held.as_str()]);
    let answers = runners
        .check_cached("torbox", AccountId::new(), &queries)
        .await
        .expect("within the budget");
    assert_eq!(answers.len(), 300);
    assert_eq!(answers[7].state, CacheState::Cached);
    assert_eq!(
        answers
            .iter()
            .filter(|answer| answer.state == CacheState::Cached)
            .count(),
        1
    );
    let requests = host.requests();
    assert_eq!(
        requests.len(),
        3,
        "the adapter splits by a hundred, one request each"
    );
    for request in &requests {
        let asked = request
            .query
            .iter()
            .find(|(name, _)| name == "hash")
            .map(|(_, value)| value.split(',').count());
        assert_eq!(asked, Some(100), "{}", request.path);
    }
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
                            "access_token"
                                | "refresh_token"
                                | "token"
                                | "api_key"
                                | "client_secret"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    // TorBox states a per-account value beside every job it creates.
                    if name == "auth_id"
                        && let Some(id) = inner.as_str()
                    {
                        assert!(
                            id.starts_with("REDACTED"),
                            "{path:?} carries a real auth id"
                        );
                    }
                    if name == "hash"
                        && let Some(hash) = inner.as_str()
                    {
                        assert!(
                            hash.eq_ignore_ascii_case(HASH)
                                || hash == "{{hash}}"
                                || hash.chars().all(|character| character == '0'),
                            "{path:?} carries a real hash: {hash}"
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
                    text.contains("REDACTED") || text.starts_with("https://api.torbox.app/"),
                    "{path:?} carries a live link: {text}"
                );
            }
            _ => {}
        }
    }
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torbox_jobs");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} fixtures were checked");
}

/// The plugin reaches only the API it needs, with only the credential it needs, and claims the
/// provider row its two siblings already share.
#[test]
fn the_plugin_reaches_only_the_part_of_torbox_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["torbox".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["api.torbox.app".to_owned()].as_slice()
    );
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["torbox_api_key".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}
