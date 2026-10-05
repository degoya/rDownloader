use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{AccountId, FailureKind};
use rd_plugin_host::extension::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, RemoteJobArtifact, RemoteJobEntry,
    RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource, RemoteJobWork,
};

use super::{
    CHOICE_NOT_KEPT, EMPTY, MAX_ARTIFACTS, NO_PLUGIN, PollOutcome, RemoteJobDriver,
    RemoteJobRunners, RunnerInfo, StartOutcome, UNSPECIFIED,
};

/// A stand-in plugin: what it claims, what it answers, and the record of what it was
/// asked.
struct Fake {
    claims: bool,
    /// The next poll answer; `None` traps.
    progress: Mutex<Option<RemoteJobProgress>>,
    asked: Mutex<Vec<&'static str>>,
}

impl Fake {
    fn answering(progress: RemoteJobProgress) -> Self {
        Self {
            claims: true,
            progress: Mutex::new(Some(progress)),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn trapping() -> Self {
        Self {
            claims: true,
            progress: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RemoteJobDriver for Fake {
    async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
        self.asked.lock().expect("asked").push("claims");
        Ok(self.claims)
    }

    async fn identify(
        &self,
        _source: &RemoteJobSource,
    ) -> Result<Result<String, RemoteJobRefusal>> {
        self.asked.lock().expect("asked").push("identify");
        Ok(Ok("c8f1a0b2".to_owned()))
    }

    async fn submit(
        &self,
        _account: AccountId,
        _source: &RemoteJobSource,
        _content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        self.asked.lock().expect("asked").push("submit");
        anyhow::bail!("out of fuel")
    }

    async fn adopt(
        &self,
        _account: AccountId,
        _content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        Ok(Ok(None))
    }

    async fn poll(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        self.asked.lock().expect("asked").push("poll");
        match self.progress.lock().expect("progress").take() {
            Some(progress) => Ok(Ok(progress)),
            None => anyhow::bail!("out of fuel"),
        }
    }

    async fn choose(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
        _chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }

    async fn discard(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }
}

const PLUGIN: &str = "019d0000-0000-7000-8000-00000000011d";

fn build(fake: Fake, claims: &[&str]) -> RemoteJobRunners {
    RemoteJobRunners::from_drivers(vec![(
        RunnerInfo {
            plugin_id: PLUGIN.to_owned(),
            name: "Fake torrents".to_owned(),
            claims: claims.iter().map(|claim| (*claim).to_owned()).collect(),
        },
        Box::new(fake),
    )])
}

fn magnet() -> RemoteJobSource {
    RemoteJobSource::Magnet("magnet:?xt=urn:btih:c8f1a0b2".to_owned())
}

fn handle() -> RemoteJobHandle {
    RemoteJobHandle {
        remote_id: "XKCD123".to_owned(),
        account_id: AccountId::new().to_string(),
        job_state: None,
    }
}

fn artifact(url: &str, file_name: Option<&str>, package_hint: Option<&str>) -> RemoteJobArtifact {
    RemoteJobArtifact {
        url: url.to_owned(),
        file_name: file_name.map(str::to_owned),
        size: Some(7),
        package_hint: package_hint.map(str::to_owned),
    }
}

fn ready(artifacts: Vec<RemoteJobArtifact>) -> RemoteJobProgress {
    RemoteJobProgress::Ready { artifacts }
}

/// A new job is routed by the provider slug the account carries; an existing row by the
/// plugin id it names. Two keys, because they answer two different questions.
#[tokio::test]
async fn a_new_job_is_routed_by_provider_and_an_existing_one_by_plugin_id() {
    let runners = build(
        Fake::answering(RemoteJobProgress::Working(RemoteJobWork::default())),
        &["realdebrid"],
    );
    assert_eq!(
        runners.providers().into_iter().collect::<Vec<_>>(),
        vec!["realdebrid".to_owned()]
    );
    // The slug is matched case-insensitively, like everywhere else an account's provider
    // is compared.
    assert_eq!(
        runners.identify("RealDebrid", &magnet()).await,
        StartOutcome::Identified {
            plugin_id: PLUGIN.to_owned(),
            content_key: "c8f1a0b2".to_owned(),
        }
    );
    // A provider nobody claims is a refusal with a code, not a silent nothing.
    assert!(matches!(
        runners.identify("premiumize", &magnet()).await,
        StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN && !refusal.retryable
    ));
    // A row naming a plugin that is not installed any more fails the same way.
    assert!(matches!(
        runners.poll("019d0000-0000-7000-8000-0000000000ff", AccountId::new(), &handle()).await,
        PollOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN
    ));
    assert!(matches!(
        runners.poll(PLUGIN, AccountId::new(), &handle()).await,
        PollOutcome::Working(_)
    ));
    assert_eq!(runners.plugin_name(PLUGIN), Some("Fake torrents"));
}

/// A source the plugin does not take is not identified either: `identify` is only asked
/// of something the plugin has said is its own.
#[tokio::test]
async fn a_source_the_plugin_does_not_take_is_not_claimed_and_never_identified() {
    let fake = Fake {
        claims: false,
        progress: Mutex::new(None),
        asked: Mutex::new(Vec::new()),
    };
    let runners = build(fake, &["realdebrid"]);
    assert_eq!(
        runners.identify("realdebrid", &magnet()).await,
        StartOutcome::NotClaimed
    );
}

/// The offer and the refusal read one table (RD-120-23).
///
/// `providers()` is what the remote-jobs form offers accounts for, and `identify` is
/// what refuses a provider nobody claims. The two have to be the same table, or the form
/// offers what the submit rejects -- which is exactly the defect this job was opened
/// for. Two providers with a plugin and one without, because with a single provider a
/// correct answer and a wrong one look identical.
///
/// And `remote_job.no_plugin` stays: a plugin can be removed between the form being
/// drawn and the button being pressed, so the refusal is still the right answer. It
/// becomes rare, not unnecessary.
#[tokio::test]
async fn the_offer_and_the_refusal_read_one_table() {
    let runners = RemoteJobRunners::from_drivers(vec![
        (
            RunnerInfo {
                plugin_id: PLUGIN.to_owned(),
                name: "Fake torrents".to_owned(),
                claims: vec!["realdebrid".to_owned()],
            },
            Box::new(Fake::answering(RemoteJobProgress::Working(
                RemoteJobWork::default(),
            ))),
        ),
        (
            RunnerInfo {
                plugin_id: "019d0000-0000-7000-8000-00000000013f".to_owned(),
                name: "Fake transfers".to_owned(),
                claims: vec!["premiumize".to_owned()],
            },
            Box::new(Fake::answering(RemoteJobProgress::Working(
                RemoteJobWork::default(),
            ))),
        ),
    ]);
    assert_eq!(
        runners.providers().into_iter().collect::<Vec<_>>(),
        vec!["premiumize".to_owned(), "realdebrid".to_owned()],
        "both installed plugins are offered, and only they"
    );
    for offered in runners.providers() {
        assert!(
            !matches!(
                runners.identify(&offered, &magnet()).await,
                StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN
            ),
            "{offered} is offered and refused at once"
        );
    }
    // A service with no plugin is neither offered nor accepted, and the refusal names
    // the code the form's filter exists to make rare.
    assert!(!runners.providers().contains("ddownload"));
    assert!(matches!(
        runners.identify("ddownload", &magnet()).await,
        StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN && !refusal.retryable
    ));
}

/// A plugin that claims no provider names no account it could run for, so it is left out
/// rather than offered for everything.
#[test]
fn a_plugin_claiming_no_provider_is_left_out() {
    let runners = build(Fake::trapping(), &[]);
    assert!(runners.is_empty());
    assert!(runners.providers().is_empty());
}

/// What a finished job hands back is reduced to addresses the LinkGrabber may take:
/// http(s) only, names and hints that cannot carry a path out of their folder, and no
/// more than the review list can hold.
#[tokio::test]
async fn only_http_addresses_reach_the_link_grabber() {
    let runners = build(
        Fake::answering(ready(vec![
            artifact(
                "https://real-debrid.com/d/REDACTED01",
                Some("ep01.mkv"),
                Some("Show/Season 1"),
            ),
            artifact("ftp://real-debrid.com/d/REDACTED02", Some("ep02.mkv"), None),
            artifact("not an address", None, None),
            artifact(
                "http://real-debrid.com/d/REDACTED03",
                Some("a\\b.mkv"),
                Some("../escape"),
            ),
        ])),
        &["realdebrid"],
    );
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), 2);
    assert_eq!(
        artifacts[0].url.as_str(),
        "https://real-debrid.com/d/REDACTED01"
    );
    assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
    assert_eq!(artifacts[0].package_hint.as_deref(), Some("Show/Season 1"));
    assert_eq!(artifacts[0].size, Some(7));
    assert_eq!(artifacts[1].file_name.as_deref(), Some("ab.mkv"));
    assert_eq!(artifacts[1].package_hint.as_deref(), Some("escape"));

    // Finished with nothing usable is a refusal, not an empty package.
    let runners = build(
        Fake::answering(ready(vec![artifact("magnet:?xt=urn:btih:x", None, None)])),
        &["realdebrid"],
    );
    assert!(matches!(
        runners.poll(PLUGIN, AccountId::new(), &handle()).await,
        PollOutcome::Refused(ref refusal) if refusal.code == EMPTY && !refusal.retryable
    ));

    // And the list is bounded, whatever the plugin said.
    let many: Vec<RemoteJobArtifact> = (0..MAX_ARTIFACTS + 100)
        .map(|index| artifact(&format!("https://real-debrid.com/d/{index}"), None, None))
        .collect();
    let runners = build(Fake::answering(ready(many)), &["realdebrid"]);
    let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected the addresses");
    };
    assert_eq!(artifacts.len(), MAX_ARTIFACTS);
}

/// A refusal carries the one thing the sweep asks of it -- wait or end -- and the wait the
/// provider suggested, so the host can clamp it rather than guess.
#[tokio::test]
async fn a_refusal_says_whether_waiting_could_change_it() {
    let refused = |category: FailureKind, code: Option<&str>| {
        RemoteJobProgress::Failed(RemoteJobRefusal {
            code: code.map(str::to_owned),
            message: "as the plugin put it".to_owned(),
            category,
        })
    };
    let rate_limited = build(
        Fake::answering(refused(
            FailureKind::RateLimited {
                retry_after_seconds: Some(120),
            },
            Some("realdebrid_torrents.rate_limited"),
        )),
        &["realdebrid"],
    );
    let PollOutcome::Refused(refusal) =
        rate_limited.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert!(refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, Some(120));
    assert_eq!(refusal.code, "realdebrid_torrents.rate_limited");
    assert_eq!(refusal.message, "as the plugin put it");

    // The provider ended the job: nothing to wait for.
    let ended = build(
        Fake::answering(refused(
            FailureKind::Permanent,
            Some("realdebrid_torrents.torrent_dead"),
        )),
        &["realdebrid"],
    );
    let PollOutcome::Refused(refusal) = ended.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert!(!refusal.retryable);
    assert_eq!(refusal.retry_after_seconds, None);

    // A refusal without a code still gets one the interface can translate.
    let unnamed = build(
        Fake::answering(refused(FailureKind::Offline, None)),
        &["realdebrid"],
    );
    let PollOutcome::Refused(refusal) = unnamed.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.code, UNSPECIFIED);
}

/// A guest that trapped, ran out of fuel or timed out says nothing about the job; the
/// call ends under one code rather than being retried against a plugin that cannot finish.
#[tokio::test]
async fn a_plugin_that_traps_ends_the_call_under_one_code() {
    let runners = build(Fake::trapping(), &["realdebrid"]);
    assert!(matches!(
        runners.poll(PLUGIN, AccountId::new(), &handle()).await,
        PollOutcome::Refused(ref refusal) if refusal.code == UNSPECIFIED && !refusal.retryable
    ));
    let refusal = runners
        .submit(PLUGIN, AccountId::new(), &magnet(), "c8f1a0b2")
        .await
        .expect_err("a trap is a refusal");
    assert_eq!(refusal.code, UNSPECIFIED);
    assert!(!refusal.retryable);
}

/// A question with nothing to choose from is not a question a person can answer.
#[tokio::test]
async fn a_question_with_nothing_to_choose_from_is_refused() {
    let empty = build(
        Fake::answering(RemoteJobProgress::AwaitingChoice {
            entries: Vec::new(),
        }),
        &["realdebrid"],
    );
    assert!(matches!(
        empty.poll(PLUGIN, AccountId::new(), &handle()).await,
        PollOutcome::Refused(ref refusal) if refusal.code == EMPTY
    ));
    let asked = build(
        Fake::answering(RemoteJobProgress::AwaitingChoice {
            entries: vec![RemoteJobEntry {
                id: 3,
                path: "Show/ep01.mkv".to_owned(),
                size: Some(10),
                selected: true,
            }],
        }),
        &["realdebrid"],
    );
    let PollOutcome::AwaitingChoice(entries) =
        asked.poll(PLUGIN, AccountId::new(), &handle()).await
    else {
        panic!("expected a question");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, 3);
    assert_eq!(entries[0].path, "Show/ep01.mkv");
    assert!(entries[0].selected);
}

/// RD-120-35: once a question was answered, asking it again ends the job under its own
/// code instead of reopening it, and every other answer passes through untouched.
#[tokio::test]
async fn a_question_asked_again_after_its_answer_ends_the_job() {
    let asked_again = build(
        Fake::answering(RemoteJobProgress::AwaitingChoice {
            entries: vec![RemoteJobEntry {
                id: 3,
                path: "Show/ep01.mkv".to_owned(),
                size: Some(10),
                selected: true,
            }],
        }),
        &["realdebrid"],
    );
    let PollOutcome::Refused(refusal) = asked_again
        .poll_answered(PLUGIN, AccountId::new(), &handle())
        .await
    else {
        panic!("an answered question must not be passed on as a new one");
    };
    assert_eq!(refusal.code, CHOICE_NOT_KEPT);
    assert!(
        !refusal.retryable,
        "waiting cannot make the provider remember"
    );

    let moving = build(
        Fake::answering(RemoteJobProgress::Preparing {
            retry_after_seconds: Some(7),
        }),
        &["realdebrid"],
    );
    assert_eq!(
        moving
            .poll_answered(PLUGIN, AccountId::new(), &handle())
            .await,
        PollOutcome::Preparing {
            retry_after_seconds: Some(7)
        }
    );
}

/// A stand-in cache (RD-130-11): the kinds it names, how often it was asked for them, and
/// every batch that reached it. Answers `Cached` with the address as the name, so the
/// order of the answers can be read back.
struct CacheFake {
    kinds: Vec<CacheKind>,
    kinds_asked: Mutex<usize>,
    batches: Mutex<Vec<usize>>,
}

impl CacheFake {
    fn naming(kinds: &[CacheKind]) -> Self {
        Self {
            kinds: kinds.to_vec(),
            kinds_asked: Mutex::new(0),
            batches: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RemoteJobDriver for std::sync::Arc<CacheFake> {
    async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
        Ok(true)
    }

    async fn identify(
        &self,
        _source: &RemoteJobSource,
    ) -> Result<Result<String, RemoteJobRefusal>> {
        Ok(Ok("c8f1a0b2".to_owned()))
    }

    async fn submit(
        &self,
        _account: AccountId,
        _source: &RemoteJobSource,
        _content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        anyhow::bail!("a cache check never submits")
    }

    async fn adopt(
        &self,
        _account: AccountId,
        _content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        Ok(Ok(None))
    }

    async fn poll(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        anyhow::bail!("a cache check never polls")
    }

    async fn choose(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
        _chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }

    async fn discard(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }

    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        *self.kinds_asked.lock().expect("kinds") += 1;
        Ok(self.kinds.clone())
    }

    async fn check_cached(
        &self,
        _account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        self.batches.lock().expect("batches").push(queries.len());
        Ok(Ok(queries
            .iter()
            .map(|query| CacheAnswer {
                state: CacheState::Cached,
                file_name: match &query.source {
                    RemoteJobSource::Address(address) | RemoteJobSource::Magnet(address) => {
                        Some(address.clone())
                    }
                    RemoteJobSource::Container(_) => None,
                },
                size: None,
            })
            .collect()))
    }
}

fn cache_runners(slug: &str, fake: &std::sync::Arc<CacheFake>) -> RemoteJobRunners {
    RemoteJobRunners::from_drivers(vec![(
        RunnerInfo {
            plugin_id: PLUGIN.to_owned(),
            name: "Fake cache".to_owned(),
            claims: vec![slug.to_owned()],
        },
        Box::new(std::sync::Arc::clone(fake)),
    )])
}

fn hoster_query(index: usize) -> CacheQuery {
    CacheQuery {
        source: RemoteJobSource::Address(format!("https://hoster.example/f/{index}")),
        kind: CacheKind::Hoster,
    }
}

/// RD-130-11: 250 queries reach the plugin as 100, 100 and 50, and every answer comes
/// back at the position of the query it answers.
#[tokio::test]
async fn a_cache_check_is_split_into_batches_and_answered_in_order() {
    let fake = std::sync::Arc::new(CacheFake::naming(&[CacheKind::Hoster]));
    let runners = cache_runners("torbox", &fake);
    let queries: Vec<CacheQuery> = (0..250).map(hoster_query).collect();
    let answers = runners
        .check_cached("TorBox", AccountId::new(), &queries)
        .await
        .expect("answers");
    assert_eq!(*fake.batches.lock().expect("batches"), vec![100, 100, 50]);
    assert_eq!(answers.len(), 250);
    for (index, answer) in answers.iter().enumerate() {
        assert_eq!(
            answer.file_name.as_deref(),
            Some(format!("https://hoster.example/f/{index}").as_str())
        );
    }
}

/// A kind the plugin did not name never reaches it, and answers `Unknown` in its place.
#[tokio::test]
async fn a_kind_the_plugin_did_not_name_never_reaches_it() {
    let fake = std::sync::Arc::new(CacheFake::naming(&[CacheKind::Torrent]));
    let runners = cache_runners("premiumize", &fake);
    let queries = vec![
        hoster_query(0),
        CacheQuery {
            source: RemoteJobSource::Magnet("magnet:?xt=urn:btih:c8f1a0b2".to_owned()),
            kind: CacheKind::Torrent,
        },
        hoster_query(2),
    ];
    let answers = runners
        .check_cached("premiumize", AccountId::new(), &queries)
        .await
        .expect("answers");
    assert_eq!(*fake.batches.lock().expect("batches"), vec![1]);
    assert_eq!(answers[0], CacheAnswer::unknown());
    assert_eq!(answers[1].state, CacheState::Cached);
    assert_eq!(answers[2], CacheAnswer::unknown());
}

/// A plugin with no cache kinds is never asked, is not listed as a cache provider, and is
/// asked for its kinds once per load however often the check runs.
#[tokio::test]
async fn a_plugin_without_cache_kinds_is_never_asked_and_kinds_are_asked_once() {
    let fake = std::sync::Arc::new(CacheFake::naming(&[]));
    let runners = cache_runners("realdebrid", &fake);
    assert!(runners.cache_providers().await.is_empty());
    for _ in 0..3 {
        let answers = runners
            .check_cached("realdebrid", AccountId::new(), &[hoster_query(0)])
            .await
            .expect("answers");
        assert_eq!(answers, vec![CacheAnswer::unknown()]);
    }
    assert!(fake.batches.lock().expect("batches").is_empty());
    assert_eq!(*fake.kinds_asked.lock().expect("kinds"), 1);

    let named = std::sync::Arc::new(CacheFake::naming(&[
        CacheKind::Usenet,
        CacheKind::Torrent,
        CacheKind::Usenet,
    ]));
    let runners = cache_runners("torbox", &named);
    assert_eq!(
        runners.cache_providers().await,
        vec![(
            "torbox".to_owned(),
            vec![CacheKind::Torrent, CacheKind::Usenet]
        )]
    );
    // A provider nobody claims is a refusal with a code, never an empty answer that
    // would read as "asked and not held".
    assert!(matches!(
        runners.check_cached("offcloud", AccountId::new(), &[hoster_query(0)]).await,
        Err(ref refusal) if refusal.code == NO_PLUGIN
    ));
}
