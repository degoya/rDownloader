//! The whole way through, against a mock provider (RD-108-03).
//!
//! The provider answers at the adapter boundary: every call that would reach a network is
//! recorded and scripted, every call that is local runs for real. What these tests prove is
//! the order of the writes -- the row before the request, the attempt before the call, the
//! identifier as the very next thing -- and what a restart, a question and a finished job do
//! to the row. A run against a real Real-Debrid account is *not* claimed here; there is none
//! in this checkout.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rd_core::{AccountId, FailureKind, RemoteJobId, RemoteJobState};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_ext::{PollOutcome, RemoteJobDriver, RemoteJobRunners, RunnerInfo};
use rd_plugin_host::extension::{
    RemoteJobArtifact, RemoteJobEntry, RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal,
    RemoteJobSource, RemoteJobWork,
};

use super::{
    ChoiceOutcome, DiscardOutcome, RemoteJobService, SUBMIT_UNCONFIRMED, SubmitOutcome, backoff,
};

const PLUGIN: &str = "019d0000-0000-7000-8000-00000000011d";
const MAGNET: &str = "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=example";
const KEY: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

/// A host that can do nothing. No component is ever instantiated here.
pub(super) struct NoHost;

#[async_trait]
impl ResolverHost for NoHost {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        _request: HostHttpRequest,
    ) -> Result<HostHttpResponse, rd_core::Failure> {
        Err(rd_core::Failure::new(
            FailureKind::Unsupported,
            "no host in this test".to_owned(),
        ))
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

/// The mock provider. Every network-shaped call is recorded, every answer is scripted, and
/// the local ones -- `claims`, `identify` -- are answered from the source alone, as a real
/// plugin answers them.
#[derive(Default)]
struct MockProvider {
    submits: Mutex<VecDeque<Result<RemoteJobHandle, RemoteJobRefusal>>>,
    adopts: Mutex<VecDeque<Option<RemoteJobHandle>>>,
    polls: Mutex<VecDeque<RemoteJobProgress>>,
    calls: Mutex<Vec<String>>,
    submitted: Mutex<Vec<RemoteJobSource>>,
}

impl MockProvider {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    fn submitted(&self) -> Vec<RemoteJobSource> {
        self.submitted.lock().expect("submitted").clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().expect("calls").push(call.into());
    }

    fn will_submit(&self, answer: Result<RemoteJobHandle, RemoteJobRefusal>) {
        self.submits.lock().expect("submits").push_back(answer);
    }

    fn will_adopt(&self, answer: Option<RemoteJobHandle>) {
        self.adopts.lock().expect("adopts").push_back(answer);
    }

    fn will_answer(&self, progress: RemoteJobProgress) {
        self.polls.lock().expect("polls").push_back(progress);
    }
}

struct Driver(Arc<MockProvider>);

#[async_trait]
impl RemoteJobDriver for Driver {
    async fn claims(&self, source: &RemoteJobSource) -> Result<bool> {
        Ok(match source {
            RemoteJobSource::Magnet(address) => address.starts_with("magnet:"),
            // The third shape (RD-120-20): a plain address the provider fetches itself.
            RemoteJobSource::Address(address) => address.starts_with("https://"),
            // A bencoded dictionary, which is as far as a mock needs to read a `.torrent`.
            RemoteJobSource::Container(bytes) => bytes.first() == Some(&b'd'),
        })
    }

    async fn identify(&self, source: &RemoteJobSource) -> Result<Result<String, RemoteJobRefusal>> {
        if let RemoteJobSource::Address(address) = source {
            // A different key rule for a different shape, prefixed so it can never collide
            // with an info hash: both live under one unique index.
            return Ok(Ok(format!("url:{address}")));
        }
        if let RemoteJobSource::Container(bytes) = source {
            return Ok(Ok(format!("file:{}", hex::encode(bytes))));
        }
        let RemoteJobSource::Magnet(address) = source else {
            return Ok(Err(refusal(FailureKind::Unsupported, "not a magnet")));
        };
        let key = address.split_once("btih:").map(|(_, rest)| {
            rest.split('&')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        });
        Ok(key.ok_or_else(|| refusal(FailureKind::Unsupported, "not a magnet")))
    }

    async fn submit(
        &self,
        _account: AccountId,
        source: &RemoteJobSource,
        _content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        self.0.record("submit");
        // What actually crossed into the driver, so a test can tell an address that was
        // rehydrated from the row apart from one that merely looked right on the way in.
        self.0
            .submitted
            .lock()
            .expect("submitted")
            .push(source.clone());
        Ok(self
            .0
            .submits
            .lock()
            .expect("submits")
            .pop_front()
            .expect("a scripted submit answer"))
    }

    async fn adopt(
        &self,
        _account: AccountId,
        _content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        self.0.record("adopt");
        Ok(Ok(self
            .0
            .adopts
            .lock()
            .expect("adopts")
            .pop_front()
            .flatten()))
    }

    async fn poll(
        &self,
        _account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        self.0.record(format!("poll {}", handle.remote_id));
        Ok(Ok(self
            .0
            .polls
            .lock()
            .expect("polls")
            .pop_front()
            .unwrap_or(RemoteJobProgress::Preparing {
                retry_after_seconds: None,
            })))
    }

    async fn choose(
        &self,
        _account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        self.0
            .record(format!("choose {} {chosen:?}", handle.remote_id));
        Ok(Ok(()))
    }

    async fn discard(
        &self,
        _account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        self.0.record(format!("discard {}", handle.remote_id));
        Ok(Ok(()))
    }
}

fn refusal(category: FailureKind, message: &str) -> RemoteJobRefusal {
    RemoteJobRefusal {
        code: Some("mock.refused".to_owned()),
        message: message.to_owned(),
        category,
    }
}

fn handle(remote_id: &str) -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: remote_id.to_owned(),
        account_id: "set by the host".to_owned(),
        job_state: None,
    }
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet(MAGNET.to_owned())
}

/// Runners over the mock, as the installer would have built them.
fn runners_for(provider: &Arc<MockProvider>) -> RemoteJobRunners {
    RemoteJobRunners::from_drivers(vec![(
        RunnerInfo {
            plugin_id: PLUGIN.to_owned(),
            name: "Mock torrents".to_owned(),
            claims: vec!["realdebrid".to_owned()],
        },
        Box::new(Driver(Arc::clone(provider))),
    )])
}

/// A database, an account on the provider the mock claims, and a service driving the mock.
struct Harness {
    _directory: tempfile::TempDir,
    database: rd_db::Database,
    account: AccountId,
    service: RemoteJobService,
}

async fn harness(provider: &Arc<MockProvider>) -> Harness {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("remote-jobs.sqlite3"))
        .await
        .expect("database");
    let account = database
        .create_account(rd_db::NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let service = RemoteJobService::detached(
        database.clone(),
        directory.path().join("plugins"),
        Arc::new(NoHost),
        runners_for(provider),
    );
    Harness {
        _directory: directory,
        database,
        account: account.id,
        service,
    }
}

impl Harness {
    async fn start(&self) -> RemoteJobId {
        match self
            .service
            .submit(self.account, magnet())
            .await
            .expect("submit")
        {
            SubmitOutcome::Started(job) => job.id,
            other => panic!("expected a started job, got {other:?}"),
        }
    }

    async fn job(&self, id: RemoteJobId) -> rd_core::RemoteJob {
        self.database
            .remote_job(id)
            .await
            .expect("read")
            .expect("the row")
    }

    async fn sweep(&self, now: DateTime<Utc>) {
        self.service.sweep_once(now).await.expect("sweep");
    }
}

fn later(now: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
    now + chrono::Duration::seconds(seconds)
}

/// The second acceptance criterion: two submits of the same content are one row, and the
/// guard fires before the first network call -- the provider was asked nothing at all.
#[tokio::test]
async fn a_second_submit_of_the_same_content_is_the_same_job_and_reaches_no_provider() {
    let provider = Arc::new(MockProvider::default());
    let harness = harness(&provider).await;

    let first = harness.start().await;
    let second = harness
        .service
        .submit(harness.account, magnet())
        .await
        .expect("submit");
    let SubmitOutcome::AlreadyOurs(existing) = second else {
        panic!("expected the existing job, got {second:?}");
    };
    assert_eq!(existing.id, first);
    // The same content in another spelling is the same key, so the same job.
    let SubmitOutcome::AlreadyOurs(_) = harness
        .service
        .submit(
            harness.account,
            RemoteJobSource::Magnet(MAGNET.to_ascii_lowercase()),
        )
        .await
        .expect("submit")
    else {
        panic!("expected the existing job");
    };

    let jobs = harness
        .database
        .remote_jobs(harness.account)
        .await
        .expect("list");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].content_key, KEY);
    assert_eq!(jobs[0].state, RemoteJobState::Submitting);
    assert!(
        provider.calls().is_empty(),
        "nothing reached the provider: {:?}",
        provider.calls()
    );

    // And something the plugin does not take writes nothing either.
    assert_eq!(
        harness
            .service
            .submit(
                harness.account,
                RemoteJobSource::Magnet("https://example.org/not-a-magnet".to_owned())
            )
            .await
            .expect("submit"),
        SubmitOutcome::NotClaimed
    );
}

/// The first acceptance criterion, as far as a mock can carry it: the row is submitted, the
/// identifier is written, the job is polled through its states and ends as one LinkGrabber
/// package with the row pointing at it -- and a tick later nothing happens again.
#[tokio::test]
async fn a_magnet_ends_as_one_package_in_the_link_grabber_and_only_one() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE01")));
    provider.will_answer(RemoteJobProgress::Working(RemoteJobWork {
        progress_permille: Some(425),
        speed_bytes_per_second: Some(1_000),
        seconds_remaining: None,
    }));
    provider.will_answer(RemoteJobProgress::Ready {
        artifacts: vec![
            RemoteJobArtifact {
                url: "https://real-debrid.com/d/REDACTED01".to_owned(),
                file_name: Some("ep01.mkv".to_owned()),
                size: Some(10),
                package_hint: Some("Example".to_owned()),
            },
            RemoteJobArtifact {
                url: "https://real-debrid.com/d/REDACTED02".to_owned(),
                file_name: Some("ep02.mkv".to_owned()),
                size: None,
                package_hint: Some("Example".to_owned()),
            },
        ],
    });
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();

    // Tick one: the attempt is counted, the source goes over, the identifier comes back.
    harness.sweep(now).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Preparing);
    assert_eq!(job.remote_id.as_deref(), Some("REMOTE01"));
    assert_eq!(job.submit_attempts, 1);
    assert!(job.next_poll_at.expect("polled again") > now);
    assert_eq!(provider.calls(), vec!["submit".to_owned()]);

    // Not due yet: the host owns the clock.
    harness.sweep(later(now, 1)).await;
    assert_eq!(provider.calls().len(), 1);

    // Tick two: working, with the progress the provider stated.
    harness.sweep(later(now, 20)).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Working);
    assert_eq!(job.progress_permille, Some(425));

    // Tick three: finished. One batch, one package, and the row points at it.
    harness.sweep(later(now, 60)).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Ready);
    assert_eq!(job.next_poll_at, None);
    assert_eq!(job.progress_permille, Some(1_000));
    let packages = harness
        .database
        .list_collector_packages()
        .await
        .expect("packages");
    assert_eq!(packages.len(), 1, "{packages:?}");
    assert_eq!(job.package_id, Some(packages[0].id));
    assert_eq!(
        provider.calls(),
        vec![
            "submit".to_owned(),
            "poll REMOTE01".to_owned(),
            "poll REMOTE01".to_owned()
        ]
    );

    // A finished job is not polled and hands nothing over a second time.
    harness.sweep(later(now, 3_600)).await;
    assert_eq!(provider.calls().len(), 3);
    assert_eq!(
        harness
            .database
            .list_collector_packages()
            .await
            .expect("packages")
            .len(),
        1
    );
}

/// The third acceptance criterion: a restart between the request going out and the
/// identifier coming back leaves an attempt and no id, and the next tick *adopts* before it
/// would ever submit again. The provider had the job, so nothing is created twice.
#[tokio::test]
async fn a_restart_inside_the_submit_window_adopts_before_it_submits_again() {
    let provider = Arc::new(MockProvider::default());
    provider.will_adopt(Some(handle("ORPHAN01")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    // The crash: the attempt was counted and the process died before the answer.
    harness
        .database
        .advance_remote_job(
            id,
            rd_db::AdvanceRemoteJob {
                count_submit_attempt: true,
                ..rd_db::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("counted");

    // Nothing is in memory; the first tick after the restart reads the row.
    harness.sweep(Utc::now()).await;
    let job = harness.job(id).await;
    assert_eq!(provider.calls(), vec!["adopt".to_owned()]);
    assert_eq!(job.remote_id.as_deref(), Some("ORPHAN01"));
    assert_eq!(job.state, RemoteJobState::Preparing);
    assert!(job.adoption_checked);
    assert_eq!(job.submit_attempts, 1, "no second attempt was made");
}

/// A restart creates no second transfer for the same content on the same account, at a
/// provider whose `adopt` answers nothing (RD-120-23).
///
/// `plugins/premiumize-transfers/` is that provider: `transfer/list` carries nothing derived
/// from what was handed over -- no hash, no `src`, no key -- so a transfer this installation
/// created cannot be told from a stranger's, and the plugin answers `none` rather than
/// adopting one on a guess. What is left holding the line is the row keyed by
/// `(account_id, content_key)`, migration `0065`'s `remote_jobs_content_idx`, and this is the
/// test that it holds without any help from the provider.
#[tokio::test]
async fn a_restart_creates_no_second_job_when_the_provider_can_adopt_nothing() {
    let provider = Arc::new(MockProvider::default());
    // One submit answer and no more: a second one would be the duplicate this test denies, and
    // the mock panics rather than inventing one, so the queue itself is half the assertion.
    provider.will_submit(Ok(handle("REMOTE01")));
    provider.will_adopt(None);
    let harness = harness(&provider).await;
    let id = harness.start().await;

    // The identifier came back and was written; then the process died.
    harness.sweep(Utc::now()).await;
    let job = harness.job(id).await;
    let remote_id = job.remote_id.clone().expect("an identifier");
    assert_eq!(provider.calls(), vec!["submit".to_owned()]);

    // The restart: nothing is in memory, and the row is read again. It names a job at the
    // provider, so the next thing is a poll and never a second submit -- the adoption is not
    // even reached, which is why a provider that cannot adopt loses nothing here.
    harness.sweep(Utc::now()).await;
    let job = harness.job(id).await;
    assert_eq!(job.remote_id, Some(remote_id));
    assert_eq!(job.submit_attempts, 1, "the content was handed over once");
    assert!(
        !provider.calls().contains(&"adopt".to_owned()),
        "{:?}",
        provider.calls()
    );
    assert_eq!(
        provider
            .calls()
            .iter()
            .filter(|call| *call == "submit")
            .count(),
        1
    );

    // And the same content pasted again on the same account is the job that already runs,
    // flagged rather than refused, with nothing sent anywhere.
    let SubmitOutcome::AlreadyOurs(existing) = harness
        .service
        .submit(harness.account, magnet())
        .await
        .expect("submit")
    else {
        panic!("the duplicate guard holds without the provider's help");
    };
    assert_eq!(existing.id, id);
    assert_eq!(
        provider
            .calls()
            .iter()
            .filter(|call| *call == "submit")
            .count(),
        1,
        "a second paste sends nothing"
    );
}

/// Two attempts are the ceiling. A submit the provider refused, an adoption that found
/// nothing, one more submit, and then the row fails under a code that says to look at the
/// account -- never a third `addMagnet`.
#[tokio::test]
async fn two_attempts_and_an_adoption_check_are_the_ceiling() {
    let provider = Arc::new(MockProvider::default());
    let busy = || {
        Err(refusal(
            FailureKind::Transient {
                retry_after_seconds: Some(30),
            },
            "busy",
        ))
    };
    provider.will_submit(busy());
    provider.will_submit(busy());
    provider.will_adopt(None);
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();

    harness.sweep(now).await;
    let job = harness.job(id).await;
    assert_eq!(
        job.state,
        RemoteJobState::Submitting,
        "a refusal that is worth waiting out"
    );
    assert_eq!(job.submit_attempts, 1);
    assert_eq!(job.code.as_deref(), Some("mock.refused"));
    let waited = job.next_poll_at.expect("waiting");
    assert_eq!(
        waited,
        later(now, 30),
        "the provider's own wait, inside the bounds"
    );

    harness.sweep(later(now, 31)).await;
    let job = harness.job(id).await;
    assert!(job.adoption_checked);
    assert_eq!(job.submit_attempts, 1);

    harness.sweep(later(now, 32)).await;
    let job = harness.job(id).await;
    assert_eq!(job.submit_attempts, 2);
    assert_eq!(job.state, RemoteJobState::Submitting);

    harness.sweep(later(now, 100)).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Failed);
    assert_eq!(job.code.as_deref(), Some(SUBMIT_UNCONFIRMED));
    assert_eq!(job.next_poll_at, None);
    assert_eq!(
        provider.calls(),
        vec!["submit".to_owned(), "adopt".to_owned(), "submit".to_owned()]
    );

    // And a failed row is never picked up again.
    harness.sweep(later(now, 10_000)).await;
    assert_eq!(provider.calls().len(), 3);
}

/// The fourth acceptance criterion: a job waiting for a choice is not polled, however long
/// nobody answers, and a choice puts it back on the clock.
#[tokio::test]
async fn a_job_waiting_for_a_choice_is_not_polled_until_one_is_made() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE02")));
    provider.will_answer(RemoteJobProgress::AwaitingChoice {
        entries: vec![
            RemoteJobEntry {
                id: 1,
                path: "Example/ep01.mkv".to_owned(),
                size: Some(10),
                selected: false,
            },
            RemoteJobEntry {
                id: 2,
                path: "Example/ep02.mkv".to_owned(),
                size: Some(20),
                selected: false,
            },
        ],
    });
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();

    harness.sweep(now).await;
    harness.sweep(later(now, 20)).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::AwaitingChoice);
    assert_eq!(job.entries.len(), 2);
    assert_eq!(job.next_poll_at, None);
    assert_eq!(provider.calls().len(), 2);

    // A day passes. Nothing at the provider is asked about.
    harness.sweep(later(now, 86_400)).await;
    assert_eq!(provider.calls().len(), 2);

    // A choice of nothing the job offered stays a choice of nothing.
    let ChoiceOutcome::Refused(refused) = harness.service.choose(id, &[99]).await.expect("choose")
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refused.code, "remote_job.empty_choice");
    assert_eq!(
        provider.calls().len(),
        2,
        "an empty choice never reaches the guest"
    );

    let ChoiceOutcome::Chosen(job) = harness
        .service
        .choose(id, &[2, 99, 2])
        .await
        .expect("choose")
    else {
        panic!("expected the choice to be taken");
    };
    assert_eq!(job.state, RemoteJobState::Working);
    assert_eq!(job.chosen, vec![2]);
    assert!(job.next_poll_at.is_some());
    assert_eq!(provider.calls()[2], "choose REMOTE02 [2]");

    // And now it is polled again.
    harness.sweep(later(Utc::now(), 1)).await;
    assert_eq!(provider.calls().len(), 4);
    assert_eq!(provider.calls()[3], "poll REMOTE02");

    // A second answer to a question that was already answered is refused.
    let ChoiceOutcome::Refused(refused) = harness.service.choose(id, &[1]).await.expect("choose")
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refused.code, "remote_job.not_awaiting_choice");
}

/// RD-108-04's fourth acceptance criterion, and the one that costs somebody a torrent if it is
/// wrong: nothing is deleted at the provider until a request says in so many words that it was
/// confirmed, and what a confirmed one did stays readable afterwards.
#[tokio::test]
async fn nothing_is_deleted_at_the_provider_without_an_explicit_confirmation() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE05")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    harness.sweep(Utc::now()).await;
    assert_eq!(provider.calls(), vec!["submit".to_owned()]);

    // An unconfirmed request reaches no provider and changes no row.
    let DiscardOutcome::Refused(refused) =
        harness.service.discard(id, false).await.expect("discard")
    else {
        panic!("an unconfirmed request must not delete anything");
    };
    assert_eq!(refused.code, "remote_job.not_confirmed");
    assert_eq!(
        provider.calls(),
        vec!["submit".to_owned()],
        "an unconfirmed request reached the provider"
    );
    assert_eq!(harness.job(id).await.state, RemoteJobState::Preparing);

    // A confirmed one does, and leaves the record behind rather than the row.
    let DiscardOutcome::Discarded(job) = harness.service.discard(id, true).await.expect("discard")
    else {
        panic!("a confirmed request has to delete at the provider");
    };
    assert_eq!(job.state, RemoteJobState::Discarded);
    assert_eq!(job.code.as_deref(), Some("remote_job.discarded"));
    assert_eq!(
        job.remote_id.as_deref(),
        Some("REMOTE05"),
        "the row has to keep naming the job the provider knew"
    );
    assert_eq!(job.next_poll_at, None);
    assert_eq!(provider.calls()[1], "discard REMOTE05");

    // The row is still there to be read, and a second confirmed request sends nothing.
    assert!(
        harness
            .service
            .jobs()
            .await
            .expect("list")
            .iter()
            .any(|listed| listed.id == id)
    );
    let DiscardOutcome::Refused(again) = harness.service.discard(id, true).await.expect("discard")
    else {
        panic!("a job already gone from the provider must not be deleted twice");
    };
    assert_eq!(again.code, "remote_job.already_discarded");
    assert_eq!(provider.calls().len(), 2);
}

/// RD-108-04's fifth acceptance criterion: clearing a row out of the list is a different act
/// from deleting at the provider, and it sends nothing anywhere.
#[tokio::test]
async fn removing_a_job_from_the_list_deletes_nothing_at_the_provider() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE06")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    harness.sweep(Utc::now()).await;

    assert!(harness.service.forget(id).await.expect("forget"));
    assert_eq!(
        provider.calls(),
        vec!["submit".to_owned()],
        "removing a row from the list reached the provider"
    );
    assert!(harness.service.jobs().await.expect("list").is_empty());
    // And a second removal is a plain "there is no such job", not a second request.
    assert!(!harness.service.forget(id).await.expect("forget"));
    assert_eq!(provider.calls().len(), 1);
}

/// A row the provider never named has nothing to delete there. Saying so is more use than
/// sending a request that names no job — the row itself can be cleared from the list, which is
/// the other endpoint and touches nobody's account.
#[tokio::test]
async fn a_job_the_provider_never_named_is_not_deleted_there() {
    let provider = Arc::new(MockProvider::default());
    let harness = harness(&provider).await;
    let id = harness.start().await;

    let DiscardOutcome::Refused(refused) =
        harness.service.discard(id, true).await.expect("discard")
    else {
        panic!("there is nothing at the provider to delete");
    };
    assert_eq!(refused.code, "remote_job.missing_remote_id");
    assert!(provider.calls().is_empty());
    assert!(harness.service.forget(id).await.expect("forget"));
}

/// A poll answer that arrives after the job was deleted does not bring it back. The store
/// refuses the write and the sweep reports it, rather than resurrecting a row.
#[tokio::test]
async fn a_late_answer_for_a_discarded_job_is_refused() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE03")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();
    harness.sweep(now).await;
    let snapshot = harness.job(id).await;
    assert_eq!(snapshot.state, RemoteJobState::Preparing);

    // The person deleted it while a poll was in flight.
    harness
        .database
        .advance_remote_job(
            id,
            rd_db::AdvanceRemoteJob {
                state: Some(RemoteJobState::Discarded),
                next_poll_at: Some(None),
                ..rd_db::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("discarded");

    let late = harness
        .service
        .settle(
            &snapshot,
            PollOutcome::Working(RemoteJobWork::default()),
            later(now, 20),
        )
        .await;
    assert!(late.is_err(), "the late answer was written");
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Discarded);
    assert_eq!(job.next_poll_at, None);

    // Nor is it ever due again.
    harness.sweep(later(now, 3_600)).await;
    assert_eq!(provider.calls(), vec!["submit".to_owned()]);
}

/// A plugin the row names that is not installed any more ends the job under a code, once,
/// rather than being looked for every five seconds.
#[tokio::test]
async fn a_row_whose_plugin_is_gone_fails_once() {
    let provider = Arc::new(MockProvider::default());
    let harness = harness(&provider).await;
    let id = harness.start().await;
    // Another plugin's row, as an uninstall would leave it.
    let orphan = RemoteJobService::detached(
        harness.database.clone(),
        harness._directory.path().join("plugins"),
        Arc::new(NoHost),
        RemoteJobRunners::none(),
    );
    orphan.sweep_once(Utc::now()).await.expect("sweep");
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Failed);
    assert_eq!(job.code.as_deref(), Some("remote_job.no_plugin"));
    assert!(provider.calls().is_empty());
}

/// A failure to load the plugins -- a disk error, not a missing plugin -- skips the tick and
/// is tried again on the next one. It must not be remembered as "no plugins": that would end
/// every torrent still running at the provider under a terminal code for a passing cause.
#[tokio::test]
async fn a_failure_to_load_the_plugins_skips_the_tick_and_ends_no_job() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE04")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let restarted = RemoteJobService::detached_scripted(
        harness.database.clone(),
        harness._directory.path().join("plugins"),
        Arc::new(NoHost),
        vec![
            Err("the plugin directory could not be read".to_owned()),
            Ok(runners_for(&provider)),
        ],
    );
    let now = Utc::now();

    let first = restarted.sweep_once(now).await;
    assert!(first.is_err(), "the tick reports the load failure");
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Submitting);
    assert_eq!(job.submit_attempts, 0);
    assert_eq!(job.code, None, "the row was not touched");
    assert!(provider.calls().is_empty());

    restarted
        .sweep_once(now)
        .await
        .expect("the next tick loads");
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Preparing);
    assert_eq!(job.remote_id.as_deref(), Some("REMOTE04"));
    assert_eq!(provider.calls(), vec!["submit".to_owned()]);
}

/// A refusal that names no wait doubles the row's previous one, up to the host's maximum,
/// and one successful answer starts the ladder again from the state's default.
#[tokio::test]
async fn a_refusal_that_names_no_wait_doubles_the_previous_one_and_a_success_resets_it() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE05")));
    let outage = || {
        RemoteJobProgress::Failed(refusal(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "502",
        ))
    };
    provider.will_answer(outage());
    provider.will_answer(outage());
    provider.will_answer(outage());
    provider.will_answer(RemoteJobProgress::Working(RemoteJobWork::default()));
    provider.will_answer(outage());
    let harness = harness(&provider).await;
    let id = harness.start().await;
    harness.sweep(Utc::now()).await;

    // The wait is read back off the row, so it is asserted the same way: due minus written.
    let waited = |job: &rd_core::RemoteJob| {
        ((job.next_poll_at.expect("waiting") - job.updated_at).num_milliseconds() + 500) / 1_000
    };
    for ladder in [30, 60, 120] {
        // Due on the wall clock, whatever the last wait was.
        harness.sweep(later(Utc::now(), 1_000)).await;
        let job = harness.job(id).await;
        assert_eq!(
            job.state,
            RemoteJobState::Preparing,
            "the row keeps its state"
        );
        assert_eq!(job.code.as_deref(), Some("mock.refused"));
        let seconds = waited(&job);
        assert!(
            (ladder - 1..=ladder + 1).contains(&seconds),
            "expected a wait of about {ladder}s, got {seconds}s"
        );
    }
    // One good answer clears the code, and the next outage starts over from the default
    // of the state it finds the row in -- `working`, thirty seconds, doubled once.
    harness.sweep(later(Utc::now(), 1_000)).await;
    let job = harness.job(id).await;
    assert_eq!(job.state, RemoteJobState::Working);
    assert_eq!(job.code, None);
    // The good answer was due on the tick's clock, a thousand seconds ahead of the wall, so
    // the outage after it has to be further ahead still to find the row due.
    harness.sweep(later(Utc::now(), 2_000)).await;
    let job = harness.job(id).await;
    let seconds = waited(&job);
    assert!((59..=61).contains(&seconds), "reset to 60s, got {seconds}s");
}

/// The ladder as a pure function: the doubling reads the previous wait, starts from the
/// state's default, and never passes the maximum.
#[test]
fn the_backoff_ladder_starts_at_the_default_and_stops_at_the_maximum() {
    let now = Utc::now();
    let mut job = rd_core::RemoteJob {
        id: RemoteJobId::new(),
        account_id: AccountId::new(),
        plugin_id: PLUGIN.to_owned(),
        content_key: KEY.to_owned(),
        remote_id: Some("REMOTE06".to_owned()),
        state: RemoteJobState::Preparing,
        source_kind: rd_core::RemoteJobSourceKind::Magnet,
        submit_attempts: 1,
        adoption_checked: false,
        package_id: None,
        entries: Vec::new(),
        chosen: Vec::new(),
        progress_permille: None,
        message: None,
        code: None,
        next_poll_at: Some(now),
        created_at: now,
        updated_at: now,
        job_state: None,
    };
    // No refusal recorded: twice the state's default.
    assert_eq!(backoff(&job), 30);
    // A refusal that waited 300 seconds: twice that.
    job.code = Some("mock.refused".to_owned());
    job.next_poll_at = Some(now + chrono::Duration::seconds(300));
    assert_eq!(backoff(&job), 600);
    // And the ladder stops at the maximum rather than climbing past it.
    job.next_poll_at = Some(now + chrono::Duration::seconds(600));
    assert_eq!(backoff(&job), rd_core::MAX_POLL_SECONDS);
    job.next_poll_at = Some(now + chrono::Duration::seconds(900));
    assert_eq!(backoff(&job), rd_core::MAX_POLL_SECONDS);
    // A recorded wait shorter than the default -- a provider's own five seconds -- still
    // doubles from the default, never below it.
    job.next_poll_at = Some(now + chrono::Duration::seconds(5));
    assert_eq!(backoff(&job), 30);
}

/// A job discarded while its `ready` poll was in flight gets no batch: the row is read
/// again right before the LinkGrabber is written to.
#[tokio::test]
async fn a_job_discarded_while_its_ready_poll_was_in_flight_gets_no_batch() {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE07")));
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();
    harness.sweep(now).await;
    let snapshot = harness.job(id).await;
    harness
        .database
        .advance_remote_job(
            id,
            rd_db::AdvanceRemoteJob {
                state: Some(RemoteJobState::Discarded),
                next_poll_at: Some(None),
                ..rd_db::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("discarded");

    let late = harness
        .service
        .settle(
            &snapshot,
            PollOutcome::Ready(vec![rd_plugin_ext::ReadyArtifact {
                url: "https://real-debrid.com/d/REDACTED07".parse().expect("url"),
                file_name: Some("ep01.mkv".to_owned()),
                size: None,
                package_hint: None,
            }]),
            later(now, 20),
        )
        .await;
    assert!(late.is_err(), "the addresses were handed over");
    assert!(
        harness
            .database
            .list_collector_packages()
            .await
            .expect("packages")
            .is_empty(),
        "no batch was written for a discarded job"
    );
    assert_eq!(harness.job(id).await.state, RemoteJobState::Discarded);
}

/// RD-120-20's acceptance criterion for the third shape of source: a guest that takes a plain
/// address gets one, through the host, all the way to the queue.
///
/// What this pins down is the part no contract test can: the address survives the row. It is
/// written as `source_kind = "address"` with the bytes beside it, and what the driver is
/// handed on the sweep is what came back out of the database, not what the caller passed.
#[tokio::test]
async fn an_address_runs_through_the_host_to_the_queue() {
    const ADDRESS: &str = "https://example.invalid/releases/Example.Release.mkv";
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTEURL01")));
    provider.will_answer(RemoteJobProgress::Ready {
        artifacts: vec![RemoteJobArtifact {
            url: "https://example.invalid/d/REDACTED01".to_owned(),
            file_name: Some("Example.Release.mkv".to_owned()),
            size: Some(10),
            package_hint: Some("Example".to_owned()),
        }],
    });
    let harness = harness(&provider).await;

    let SubmitOutcome::Started(job) = harness
        .service
        .submit(
            harness.account,
            RemoteJobSource::Address(ADDRESS.to_owned()),
        )
        .await
        .expect("submit")
    else {
        panic!("an address the plugin claims starts a job");
    };
    assert_eq!(job.source_kind, rd_core::RemoteJobSourceKind::Address);
    assert_eq!(job.content_key, format!("url:{ADDRESS}"));
    assert!(
        provider.calls().is_empty(),
        "identifying reaches no provider"
    );

    let now = Utc::now();
    harness.sweep(now).await;
    assert_eq!(
        provider.submitted(),
        vec![RemoteJobSource::Address(ADDRESS.to_owned())],
        "the address that reached the plugin is the one the row held"
    );
    let row = harness.job(job.id).await;
    assert_eq!(row.remote_id.as_deref(), Some("REMOTEURL01"));

    // And it finishes as a LinkGrabber package like any other job.
    harness.sweep(later(now, 60)).await;
    let row = harness.job(job.id).await;
    assert_eq!(row.state, RemoteJobState::Ready);
    assert!(row.package_id.is_some(), "no package reached the queue");

    // A second paste of the same address is the same job, and nothing is sent again.
    let SubmitOutcome::AlreadyOurs(existing) = harness
        .service
        .submit(
            harness.account,
            RemoteJobSource::Address(ADDRESS.to_owned()),
        )
        .await
        .expect("submit")
    else {
        panic!("the duplicate guard holds for an address too");
    };
    assert_eq!(existing.id, job.id);
}

/// A container handed over as base64 (RD-120-31) reaches the plugin as the bytes it was.
///
/// Until RD-120-31 nothing constructed `RemoteJobSource::Container` at all: the route took a
/// magnet or an address, and a plugin that accepted a container never received one. The
/// route decodes with the import routes' own decoder; what this pins down is the rest — the
/// bytes survive the row, and the plugin is handed what came back out of the database.
#[tokio::test]
async fn a_container_runs_through_the_host_to_the_plugin() {
    use base64::Engine;
    let torrent = b"d4:infod4:name4:testee".to_vec();
    let sent = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let decoded = crate::container_upload::decode_base64(
        &sent,
        crate::remote_job_handlers::MAX_REMOTE_JOB_CONTAINER_BYTES,
    )
    .expect("decode");
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTEFILE01")));
    let harness = harness(&provider).await;

    let SubmitOutcome::Started(job) = harness
        .service
        .submit(harness.account, RemoteJobSource::Container(decoded))
        .await
        .expect("submit")
    else {
        panic!("a container the plugin claims starts a job");
    };
    assert_eq!(job.source_kind, rd_core::RemoteJobSourceKind::Container);
    assert!(
        provider.calls().is_empty(),
        "identifying reaches no provider"
    );

    harness.sweep(Utc::now()).await;
    assert_eq!(
        provider.submitted(),
        vec![RemoteJobSource::Container(torrent)],
        "the bytes that reached the plugin are the ones the caller sent"
    );
    let row = harness.job(job.id).await;
    assert_eq!(row.remote_id.as_deref(), Some("REMOTEFILE01"));
}

/// RD-120-35, the circle this job exists to break: `awaiting-choice` -> `choose` -> `poll`
/// must not land the row in `awaiting_choice` again. A guest whose provider did not keep the
/// answer asks the same question on the next poll; the row ends under its own code instead of
/// waiting for an answer it was already given.
#[tokio::test]
async fn a_question_asked_again_after_the_answer_ends_the_job_and_never_reopens_it() {
    let question = || RemoteJobProgress::AwaitingChoice {
        entries: vec![RemoteJobEntry {
            id: 1,
            path: "Example/ep01.mkv".to_owned(),
            size: Some(10),
            selected: false,
        }],
    };
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("REMOTE35")));
    provider.will_answer(question());
    // The provider forgot: the same question once more after it was answered.
    provider.will_answer(question());
    let harness = harness(&provider).await;
    let id = harness.start().await;
    let now = Utc::now();

    harness.sweep(now).await;
    harness.sweep(later(now, 20)).await;
    assert_eq!(harness.job(id).await.state, RemoteJobState::AwaitingChoice);

    let ChoiceOutcome::Chosen(job) = harness.service.choose(id, &[1]).await.expect("choose") else {
        panic!("expected the choice to be taken");
    };
    assert_eq!(job.state, RemoteJobState::Working);

    harness.sweep(later(Utc::now(), 1)).await;
    let job = harness.job(id).await;
    assert_ne!(job.state, RemoteJobState::AwaitingChoice);
    assert_eq!(job.state, RemoteJobState::Failed);
    assert_eq!(job.code.as_deref(), Some("remote_job.choice_not_kept"));
    assert_eq!(
        job.chosen,
        vec![1],
        "the answer that was given stays on the row"
    );
    assert_eq!(
        provider.calls(),
        vec![
            "submit",
            "poll REMOTE35",
            "choose REMOTE35 [1]",
            "poll REMOTE35"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );

    // Ended is ended: nothing polls it again, and no answer reopens it.
    harness.sweep(later(Utc::now(), 86_400)).await;
    assert_eq!(provider.calls().len(), 4);
    let ChoiceOutcome::Refused(refused) = harness.service.choose(id, &[1]).await.expect("choose")
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refused.code, "remote_job.not_awaiting_choice");
}
