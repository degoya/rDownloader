//! The Real-Debrid torrent contract, driven end to end against a mock of the provider
//! (RD-108-03).
//!
//! `plugins/realdebrid-torrents/` runs as a real WebAssembly component -- the one built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-realdebrid-torrents`
//! -- and the mock stands in for `api.real-debrid.com`. It answers at the host boundary, so no
//! socket is opened, no account is needed and no request leaves the machine; and because it
//! sees each request exactly as the plugin described it, a test can assert that the account's
//! token left the plugin as the template `{{secret:realdebrid_access_token}}` and never as a
//! value.
//!
//! The plugin is driven through `RemoteJobRunners`, the adapter the sweep uses, so what is
//! proven is the pair -- guest and adapter -- and not the guest alone.
//!
//! | Case | Fixture | Outcome |
//! | --- | --- | --- |
//! | A magnet is submitted | `add_magnet.json` | a handle carrying the torrent's id |
//! | The magnet is being converted | `info_magnet_conversion.json` | `Preparing`, with a wait |
//! | Real-Debrid wants a selection | `info_waiting_files_selection.json` | `AwaitingChoice`, three entries |
//! | The files were chosen | `selectFiles` | one request, `files=1,2` |
//! | The torrent is downloading | `info_downloading.json` | `Working`, 425 permille |
//! | The poll after the choice (RD-120-35) | `info_downloading.json`, then `info_waiting_files_selection.json` | `Working`; the question asked again is refused as `remote_job.choice_not_kept` |
//! | The torrent finished | `info_downloaded.json` | `Ready`, two addresses with names and a folder |
//! | Real-Debrid ended it | `info_error.json`, `info_magnet_error.json` | a refusal that ends the job |
//! | The sign-in expired | `error_bad_token.json` | a refusal that ends the job, `auth_invalid` |
//! | The request budget is spent | `error_too_many_requests.json` + `Retry-After` | a wait carrying the header |
//! | A submit was lost in flight | `torrents.json` | adopted by its hash; a stranger's is not |
//!
//! **A run against the real provider is not claimed here.** It needs a Real-Debrid account;
//! `docs/roadmap/jobs/108-03-remote-job-der-ablauf.md` records that as open.

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

const MANIFEST: &str = include_str!("../../../plugins/realdebrid-torrents/manifest.toml");
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-00000000011d";

// The sanitised fixtures. Every identifier in them is a placeholder and every link points at
// a redacted path; `fixtures_carry_no_credential_material` is what keeps it that way.
const ADD_MAGNET: &str = include_str!("fixtures/realdebrid_torrents/add_magnet.json");
const TORRENTS: &str = include_str!("fixtures/realdebrid_torrents/torrents.json");
const INFO_CONVERSION: &str =
    include_str!("fixtures/realdebrid_torrents/info_magnet_conversion.json");
const INFO_SELECTION: &str =
    include_str!("fixtures/realdebrid_torrents/info_waiting_files_selection.json");
const INFO_DOWNLOADING: &str = include_str!("fixtures/realdebrid_torrents/info_downloading.json");
const INFO_DOWNLOADED: &str = include_str!("fixtures/realdebrid_torrents/info_downloaded.json");
const INFO_ERROR: &str = include_str!("fixtures/realdebrid_torrents/info_error.json");
const INFO_MAGNET_ERROR: &str = include_str!("fixtures/realdebrid_torrents/info_magnet_error.json");
const ERROR_BAD_TOKEN: &str = include_str!("fixtures/realdebrid_torrents/error_bad_token.json");
const ERROR_TOO_MANY: &str =
    include_str!("fixtures/realdebrid_torrents/error_too_many_requests.json");

/// The info hash every fixture carries: SHA-1 of nothing, recognisable as a placeholder.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const TORRENT_ID: &str = "REDACTEDTORRENT01";
const MAGNET: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
const API: &str = "/rest/1.0";
const TOKEN_TEMPLATE: &str = "Bearer {{secret:realdebrid_access_token}}";

/// The plugin component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-realdebrid-torrents")
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

/// The mock Real-Debrid `torrents/*` API, routed by method and path.
struct MockRealDebrid {
    /// What `torrents/info/{id}` answers next, in order; the last one repeats.
    info: Mutex<VecDeque<&'static str>>,
    /// When set, every request is answered with this status, body and `Retry-After`.
    failure: Option<(u16, &'static str, Option<&'static str>)>,
    requests: Mutex<Vec<Recorded>>,
    authorizations: Mutex<Vec<String>>,
}

impl MockRealDebrid {
    fn new(info: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            info: Mutex::new(info.iter().copied().collect()),
            failure: None,
            requests: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        })
    }

    fn failing(status: u16, body: &'static str, retry_after: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            info: Mutex::new(VecDeque::new()),
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

    fn next_info(&self) -> &'static str {
        let mut queue = self.info.lock().expect("info");
        if queue.len() > 1 {
            queue.pop_front().expect("a queued answer")
        } else {
            queue.front().copied().unwrap_or(INFO_CONVERSION)
        }
    }
}

#[async_trait]
impl ResolverHost for MockRealDebrid {
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
            query: request
                .query
                .iter()
                .map(|value| (value.name.clone(), value.value_template.clone()))
                .collect(),
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
        let info = format!("{API}/torrents/info/{TORRENT_ID}");
        let select = format!("{API}/torrents/selectFiles/{TORRENT_ID}");
        let delete = format!("{API}/torrents/delete/{TORRENT_ID}");
        match (request.method.as_str(), path.as_str()) {
            ("POST", p) if p == format!("{API}/torrents/addMagnet") => {
                answer(201, ADD_MAGNET, None)
            }
            ("PUT", p) if p == format!("{API}/torrents/addTorrent") => {
                answer(201, ADD_MAGNET, None)
            }
            ("GET", p) if p == format!("{API}/torrents") => answer(200, TORRENTS, None),
            ("GET", p) if p == info => answer(200, self.next_info(), None),
            ("POST", p) if p == select => answer(204, "", None),
            ("DELETE", p) if p == delete => answer(204, "", None),
            _ => answer(404, r#"{"error":"unknown_ressource","error_code":7}"#, None),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        reference == "realdebrid_access_token"
    }
}

fn runners(host: Arc<MockRealDebrid>, bytes: &[u8]) -> RemoteJobRunners {
    let plugin = RemoteJobPlugin::new(manifest(), bytes, Some(host)).expect("the plugin builds");
    RemoteJobRunners::from_plugins(vec![plugin])
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

fn handle() -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: TORRENT_ID.to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: None,
    }
}

#[tokio::test]
async fn the_plugin_compiles_against_the_remote_job_world() {
    let bytes = component();
    RemoteJobPlugin::new(manifest(), &bytes, None).expect("the plugin satisfies the world");
}

/// RD-130-11: no cache query is bound for Real-Debrid, whose `instantAvailability` is switched off, so the plugin names no cache kind and
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

/// The two calls that reach nothing: whether the source is a torrent at all, and the key it
/// is known by. Both spellings of one info hash are one key, which is what makes a magnet
/// copied from two sites one job and not two.
#[tokio::test]
async fn only_torrent_sources_are_claimed_and_the_key_is_the_info_hash() {
    let bytes = component();
    let host = MockRealDebrid::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);

    assert_eq!(
        runners.identify("realdebrid", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: HASH.to_owned(),
        }
    );
    assert_eq!(
        runners
            .identify(
                "realdebrid",
                &RemoteJobSource::Magnet(MAGNET_BASE32.to_owned())
            )
            .await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: HASH.to_owned(),
        }
    );
    // A hoster link is the resolver's business, and a magnet naming no BitTorrent hash is
    // nobody's.
    for foreign in [
        "https://real-debrid.com/d/REDACTEDLINK01",
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
    ] {
        assert_eq!(
            runners
                .identify("realdebrid", &RemoteJobSource::Magnet(foreign.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{foreign}"
        );
    }
    // A container is keyed by the same rule: the SHA-1 of its `info` dictionary, forty hex
    // digits, the same twice over.
    let container = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee".to_vec();
    let StartOutcome::Identified { content_key, .. } = runners
        .identify("realdebrid", &RemoteJobSource::Container(container.clone()))
        .await
    else {
        panic!("a torrent file is claimed");
    };
    assert_eq!(content_key.len(), 40);
    assert!(content_key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(content_key, HASH);
    assert!(matches!(
        runners.identify("realdebrid", &RemoteJobSource::Container(container)).await,
        StartOutcome::Identified { content_key: again, .. } if again == content_key
    ));
    // The third shape of source (RD-120-20). It crosses the contract intact and this guest
    // answers for itself: Real-Debrid's torrent endpoints take a magnet or a `.torrent` file
    // and nothing else, so an ordinary address is not claimed. Reaching the guest at all is
    // what is being shown here -- an unmatched variant would have been a trap, not a `false`.
    for address in [
        "https://example.invalid/some/file.bin",
        "http://example.invalid/some/file.nzb",
    ] {
        assert_eq!(
            runners
                .identify("realdebrid", &RemoteJobSource::Address(address.to_owned()))
                .await,
            StartOutcome::NotClaimed,
            "{address}"
        );
    }
    assert!(
        host.requests().is_empty(),
        "claiming and identifying reach nothing"
    );
}

/// The two shapes that existed before the third was added behave exactly as they did.
///
/// Stated as its own test because it is an acceptance criterion of RD-120-20 rather than an
/// implementation detail: the variant was added at the end of `job-source`, and the numbers
/// the canonical ABI gives `magnet` and `container` are unchanged by that. A guest built
/// against the old contract and one built against the new agree on what a magnet is.
#[tokio::test]
async fn the_two_older_sources_are_unaffected_by_the_third() {
    let bytes = component();
    let host = MockRealDebrid::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    assert_eq!(
        runners.identify("realdebrid", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN_ID.to_owned(),
            content_key: HASH.to_owned(),
        }
    );
    let container = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee".to_vec();
    assert!(matches!(
        runners
            .identify("realdebrid", &RemoteJobSource::Container(container))
            .await,
        StartOutcome::Identified { content_key, .. } if content_key.len() == 40
    ));
}

/// The whole way through, in the order the sweep drives it: submitted, converted, a question
/// asked and answered, downloaded, and finished as addresses with names and a folder.
#[tokio::test]
async fn a_magnet_runs_through_to_addresses_the_link_grabber_can_take() {
    let bytes = component();
    let host = MockRealDebrid::new(&[
        INFO_CONVERSION,
        INFO_SELECTION,
        INFO_DOWNLOADING,
        INFO_DOWNLOADED,
    ]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    // Submit: one request, a form body carrying the magnet, and the id the provider named.
    let handle = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect("submitted");
    assert_eq!(handle.remote_id, TORRENT_ID);
    assert_eq!(handle.account_id, account.to_string());
    let submitted = &host.requests()[0];
    assert_eq!(submitted.method, "POST");
    assert_eq!(submitted.path, format!("{API}/torrents/addMagnet"));
    assert!(
        submitted
            .body
            .starts_with("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{}",
        submitted.body
    );

    // Converting: nobody is needed, and the plugin suggests a wait the host will clamp.
    assert_eq!(
        runners.poll(PLUGIN_ID, account, &handle).await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(10)
        }
    );

    // The question: three entries, their paths reduced to places inside the job.
    let PollOutcome::AwaitingChoice(entries) = runners.poll(PLUGIN_ID, account, &handle).await
    else {
        panic!("expected the selection");
    };
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id, 1);
    assert_eq!(entries[0].path, "Example.Release/ep01.mkv");
    assert_eq!(entries[0].size, Some(10));
    assert!(!entries[0].selected);
    assert_eq!(entries[2].path, "Example.Release/Sample/sample.mkv");

    // The answer: one request naming exactly the chosen ids.
    runners
        .choose(PLUGIN_ID, account, &handle, &[2, 1])
        .await
        .expect("chosen");
    let chosen = host
        .requests()
        .last()
        .cloned()
        .expect("the selection request");
    assert_eq!(chosen.method, "POST");
    assert_eq!(
        chosen.path,
        format!("{API}/torrents/selectFiles/{TORRENT_ID}")
    );
    assert_eq!(chosen.body, "files=1,2");

    // Downloading: the provider's percent, in thousandths.
    let PollOutcome::Working(work) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the download");
    };
    assert_eq!(work.progress_permille, Some(425));
    assert_eq!(work.speed_bytes_per_second, Some(1_048_576));

    // Finished: the two selected files, each with its name and the torrent as its folder.
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN_ID, account, &handle).await else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 2);
    assert_eq!(
        artifacts[0].url.as_str(),
        "https://real-debrid.com/d/REDACTEDLINK01"
    );
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[0].size, Some(10));
    assert_eq!(
        artifacts[0].package_hint.as_deref(),
        Some("Example.Release")
    );
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ep02.mkv"));
    assert_eq!(
        artifacts[1].package_hint.as_deref(),
        Some("Example.Release")
    );

    // Nothing in the chain deleted anything.
    assert!(
        host.requests()
            .iter()
            .all(|request| request.method != "DELETE"),
        "discard is never a side effect"
    );
}

/// RD-120-35: the answer lives at the provider, so the guest needs no memory of it.
///
/// Real-Debrid is the one bundled provider that selects server-side, and it keeps the answer:
/// once `selectFiles` succeeded, `status` is no longer `waiting_files_selection`. The poll
/// after the choice therefore reads a moving torrent, not the question again -- the whole
/// reason `awaiting-choice` is kept for providers like this one and needs no contract change.
/// And should Real-Debrid ever answer with the question again, the host ends the job rather
/// than reopening a question somebody already answered.
#[tokio::test]
async fn after_the_choice_the_real_debrid_guest_reads_the_answer_back_from_the_provider() {
    let bytes = component();
    let host = MockRealDebrid::new(&[INFO_SELECTION, INFO_DOWNLOADING]);
    let answered = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();

    let PollOutcome::AwaitingChoice(_) = answered.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected the selection");
    };
    answered
        .choose(PLUGIN_ID, account, &handle(), &[1])
        .await
        .expect("chosen");
    let PollOutcome::Working(work) = answered.poll_answered(PLUGIN_ID, account, &handle()).await
    else {
        panic!("the poll after the answer must not ask again");
    };
    assert_eq!(work.progress_permille, Some(425));

    // A provider that forgot: the guest passes the question on, and the host refuses it.
    let forgetful = MockRealDebrid::new(&[INFO_SELECTION]);
    let forgot = runners(Arc::clone(&forgetful), &bytes);
    let PollOutcome::Refused(refusal) = forgot.poll_answered(PLUGIN_ID, account, &handle()).await
    else {
        panic!("an answered question must never be reopened");
    };
    assert_eq!(refusal.code, "remote_job.choice_not_kept");
    assert!(!refusal.retryable);
}

/// The token the plugin sends is the vault template, on every request, and never a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = component();
    let host = MockRealDebrid::new(&[INFO_DOWNLOADED]);
    let runners = runners(Arc::clone(&host), &bytes);
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

/// A torrent the provider ended is a refusal that ends the job, under the code that says how.
#[tokio::test]
async fn a_torrent_the_provider_ended_is_a_failure_and_not_a_wait() {
    let bytes = component();
    for (fixture, code) in [
        (INFO_ERROR, "realdebrid_torrents.torrent_failed"),
        (INFO_MAGNET_ERROR, "realdebrid_torrents.magnet_rejected"),
    ] {
        let host = MockRealDebrid::new(&[fixture]);
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

/// `error_code` 8: the sign-in is gone. Waiting does not bring it back, so the job ends and
/// the person is told to sign in again rather than watching a job poll for ever.
#[tokio::test]
async fn an_expired_sign_in_ends_the_job_under_its_own_code() {
    let bytes = component();
    let host = MockRealDebrid::failing(401, ERROR_BAD_TOKEN, None);
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "realdebrid_torrents.auth_invalid");
    assert!(!refusal.retryable);
    // The same on the way in: a submit that cannot authenticate created nothing to adopt.
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "realdebrid_torrents.auth_invalid");
    assert!(!refusal.retryable);
}

/// `error_code` 34 with `Retry-After`: the account's request budget is spent. A wait, with
/// the provider's own figure carried out for the host to clamp -- refused requests count
/// towards the cap that refused them, so asking again at once would only extend it.
#[tokio::test]
async fn a_spent_request_budget_is_a_wait_carrying_retry_after() {
    let bytes = component();
    let host = MockRealDebrid::failing(429, ERROR_TOO_MANY, Some("120"));
    let runners = runners(host, &bytes);
    let account = AccountId::new();
    let PollOutcome::Refused(refusal) = runners.poll(PLUGIN_ID, account, &handle()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, "realdebrid_torrents.rate_limited");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
    let refusal = runners
        .submit(PLUGIN_ID, account, &magnet(), HASH)
        .await
        .expect_err("refused");
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
}

/// The crash window, closed at the provider: a submit whose answer never arrived is found
/// again by its hash in the account's own list, and somebody else's torrent is not.
#[tokio::test]
async fn an_orphaned_torrent_is_adopted_by_its_hash_and_a_stranger_is_not() {
    let bytes = component();
    let host = MockRealDebrid::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let account = AccountId::new();
    let adopted = runners
        .adopt(PLUGIN_ID, account, HASH)
        .await
        .expect("listed")
        .expect("found by its hash");
    assert_eq!(adopted.remote_id, TORRENT_ID);
    let listed = &host.requests()[0];
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, format!("{API}/torrents"));
    assert_eq!(
        listed.query,
        vec![("limit".to_owned(), "100".to_owned())],
        "one page, never a walk through the whole account"
    );
    assert_eq!(
        runners
            .adopt(
                PLUGIN_ID,
                account,
                "ffffffffffffffffffffffffffffffffffffffff"
            )
            .await
            .expect("listed"),
        None
    );
}

/// An empty choice never reaches the guest: at Real-Debrid it is an error and elsewhere it
/// silently means "all of them", and neither is an answer anybody gave.
#[tokio::test]
async fn an_empty_choice_never_reaches_the_guest() {
    let bytes = component();
    let host = MockRealDebrid::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    let refusal = runners
        .choose(PLUGIN_ID, AccountId::new(), &handle(), &[])
        .await
        .expect_err("refused");
    assert_eq!(refusal.code, "remote_job.empty_choice");
    assert!(host.requests().is_empty());
}

/// `discard` reaches the provider exactly when it is called, and nothing else calls it.
#[tokio::test]
async fn discard_reaches_the_provider_only_when_called() {
    let bytes = component();
    let host = MockRealDebrid::new(&[]);
    let runners = runners(Arc::clone(&host), &bytes);
    runners
        .discard(PLUGIN_ID, AccountId::new(), &handle())
        .await
        .expect("discarded");
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(
        requests[0].path,
        format!("{API}/torrents/delete/{TORRENT_ID}")
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
                            "access_token" | "refresh_token" | "token" | "client_secret"
                        ),
                        "{path:?} carries a `{name}` field"
                    );
                    if name == "id"
                        && let Some(id) = inner.as_str()
                    {
                        assert!(id.starts_with("REDACTED"), "{path:?} carries a real id");
                    }
                    if name == "hash"
                        && let Some(hash) = inner.as_str()
                    {
                        assert!(
                            hash.eq_ignore_ascii_case(HASH) || hash.chars().all(|c| c == '0'),
                            "{path:?} carries a real hash"
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
                    text.contains("REDACTED") || text.starts_with("https://api.real-debrid.com/"),
                    "{path:?} carries a live link: {text}"
                );
            }
            _ => {}
        }
    }
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/realdebrid_torrents");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the fixture directory") {
        let path = entry.expect("a fixture").path();
        let body = std::fs::read_to_string(&path).expect("a fixture");
        let value: serde_json::Value = serde_json::from_str(&body).expect("fixture is JSON");
        walk(&value, &path);
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures were checked");
}

/// The plugin reaches only the API it needs, with only the token it needs, and claims the
/// provider row its two siblings already share.
#[test]
fn the_plugin_reaches_only_the_part_of_real_debrid_it_needs() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::RemoteJob);
    assert_eq!(manifest.id.to_string(), PLUGIN_ID);
    assert!(
        manifest.provider.is_none(),
        "the provider row is the resolver's"
    );
    let extension = manifest.extension.as_ref().expect("an extension section");
    assert_eq!(extension.claims, vec!["realdebrid".to_owned()]);
    assert_eq!(
        manifest.capabilities.domains(),
        ["api.real-debrid.com".to_owned()].as_slice()
    );
    // The access token and nothing else: submitting a magnet has no business with the
    // application the person registered.
    assert_eq!(
        manifest.capabilities.secrets,
        vec!["realdebrid_access_token".to_owned()]
    );
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.net_stream.is_none());
}
