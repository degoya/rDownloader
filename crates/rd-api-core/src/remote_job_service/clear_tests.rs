//! Clearing the list in one go, against two mock providers (RD-1200-01).
//!
//! One provider deletes what it is asked to, the other refuses every deletion. What these tests
//! hold is that the refusal stays with its own rows -- they are kept and named -- while every
//! other row is cleared, that a job still running is left out, and that clearing "here only"
//! asks no provider anything.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{
    AccountId, FailureKind, RemoteJob, RemoteJobId, RemoteJobSourceKind, RemoteJobState,
};
use rd_db::{AdvanceRemoteJob, ClaimRemoteJob};
use rd_plugin_ext::{RemoteJobDriver, RemoteJobRunners, RunnerInfo};
use rd_plugin_host::extension::{
    RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource,
};

use super::{RemoteJobClearFilter, RemoteJobService, clear::select, tests::NoHost};

const ACCEPTING: &str = "019d0000-0000-7000-8000-0000000012a1";
const REFUSING: &str = "019d0000-0000-7000-8000-0000000012a2";

/// A provider that only ever gets asked to delete. Every other call is a mistake here.
struct Provider {
    refuses: bool,
    discarded: Arc<Mutex<Vec<String>>>,
}

fn not_here() -> RemoteJobRefusal {
    RemoteJobRefusal {
        code: Some("mock.not_here".to_owned()),
        message: "this test only deletes".to_owned(),
        category: FailureKind::Unsupported,
    }
}

#[async_trait]
impl RemoteJobDriver for Provider {
    async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
        Ok(false)
    }

    async fn identify(
        &self,
        _source: &RemoteJobSource,
    ) -> Result<Result<String, RemoteJobRefusal>> {
        Ok(Err(not_here()))
    }

    async fn submit(
        &self,
        _account: AccountId,
        _source: &RemoteJobSource,
        _content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        Ok(Err(not_here()))
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
        Ok(Err(not_here()))
    }

    async fn choose(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
        _chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Err(not_here()))
    }

    async fn discard(
        &self,
        _account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        self.discarded
            .lock()
            .expect("discarded")
            .push(handle.remote_id.clone());
        if self.refuses {
            return Ok(Err(RemoteJobRefusal {
                code: Some("torbox.offline".to_owned()),
                message: "the provider did not answer".to_owned(),
                category: FailureKind::Transient {
                    retry_after_seconds: None,
                },
            }));
        }
        Ok(Ok(()))
    }
}

struct Harness {
    _directory: tempfile::TempDir,
    database: rd_db::Database,
    service: RemoteJobService,
    realdebrid: AccountId,
    torbox: AccountId,
    /// The remote ids each provider was asked to delete, in order.
    discarded: Arc<Mutex<Vec<String>>>,
}

async fn account(database: &rd_db::Database, provider: &str) -> AccountId {
    database
        .create_account(rd_db::NewAccount {
            provider: provider.to_owned(),
            label: provider.to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

async fn harness() -> Harness {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("clear.sqlite3"))
        .await
        .expect("database");
    let realdebrid = account(&database, "realdebrid").await;
    let torbox = account(&database, "torbox").await;
    let discarded = Arc::new(Mutex::new(Vec::new()));
    let runner =
        |plugin_id: &str, claim: &str, refuses: bool| -> (RunnerInfo, Box<dyn RemoteJobDriver>) {
            (
                RunnerInfo {
                    plugin_id: plugin_id.to_owned(),
                    name: format!("Mock {claim}"),
                    claims: vec![claim.to_owned()],
                },
                Box::new(Provider {
                    refuses,
                    discarded: Arc::clone(&discarded),
                }),
            )
        };
    let runners = RemoteJobRunners::from_drivers(vec![
        runner(ACCEPTING, "realdebrid", false),
        runner(REFUSING, "torbox", true),
    ]);
    let service = RemoteJobService::detached(
        database.clone(),
        directory.path().join("plugins"),
        Arc::new(NoHost),
        runners,
    );
    Harness {
        _directory: directory,
        database,
        service,
        realdebrid,
        torbox,
        discarded,
    }
}

impl Harness {
    /// A row in `state`, with a remote id when one is given. Written directly: what a sweep did
    /// to get it there is not what these tests are about.
    async fn job(
        &self,
        account: AccountId,
        state: RemoteJobState,
        remote_id: Option<&str>,
    ) -> RemoteJobId {
        let id = RemoteJobId::new();
        let plugin_id = if account == self.torbox {
            REFUSING
        } else {
            ACCEPTING
        };
        self.database
            .claim_remote_job(ClaimRemoteJob {
                id,
                account_id: account,
                plugin_id: plugin_id.to_owned(),
                content_key: id.to_string(),
                source_kind: RemoteJobSourceKind::Magnet,
                source: b"magnet:?xt=urn:btih:clear".to_vec(),
                source_name: None,
                package_id: None,
            })
            .await
            .expect("claim");
        if state != RemoteJobState::Submitting || remote_id.is_some() {
            self.database
                .advance_remote_job(
                    id,
                    AdvanceRemoteJob {
                        state: Some(state),
                        remote_id: remote_id.map(str::to_owned),
                        ..AdvanceRemoteJob::default()
                    },
                )
                .await
                .expect("advance");
        }
        id
    }

    async fn ids(&self) -> Vec<RemoteJobId> {
        let mut ids: Vec<_> = self
            .service
            .jobs()
            .await
            .expect("list")
            .into_iter()
            .map(|job| job.id)
            .collect();
        ids.sort();
        ids
    }

    fn discarded(&self) -> Vec<String> {
        self.discarded.lock().expect("discarded").clone()
    }
}

fn sorted(mut ids: Vec<RemoteJobId>) -> Vec<RemoteJobId> {
    ids.sort();
    ids
}

/// The owner's second variant: a refusing provider keeps its own row and says why, and every
/// other row goes -- the one deleted at its provider, the one that names nothing there, and
/// the one already discarded. The job still running is left out of both halves.
#[tokio::test]
async fn one_refusing_provider_keeps_its_row_and_the_rest_are_cleared() {
    let harness = harness().await;
    let deleted = harness
        .job(harness.realdebrid, RemoteJobState::Ready, Some("RD-1"))
        .await;
    let never_named = harness
        .job(harness.realdebrid, RemoteJobState::Failed, None)
        .await;
    let refused = harness
        .job(harness.torbox, RemoteJobState::Failed, Some("TB-1"))
        .await;
    let gone = harness
        .job(harness.torbox, RemoteJobState::Discarded, Some("TB-0"))
        .await;
    let running = harness
        .job(harness.realdebrid, RemoteJobState::Working, Some("RD-2"))
        .await;

    let report = harness
        .service
        .clear(&RemoteJobClearFilter::default(), true)
        .await
        .expect("clear");

    assert_eq!(report.removed(), 3, "{report:?}");
    assert_eq!(report.failed(), 1, "{report:?}");
    let refusal = report
        .results
        .iter()
        .find(|job| job.id == refused)
        .and_then(|job| job.refusal.clone())
        .expect("the refused row names its refusal");
    assert_eq!(refusal.code, "torbox.offline");
    assert_eq!(
        report.skipped.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![running]
    );
    // Only the rows that name a job at a provider reached one, each exactly once.
    let mut asked = harness.discarded();
    asked.sort();
    assert_eq!(asked, vec!["RD-1".to_owned(), "TB-1".to_owned()]);
    assert_eq!(harness.ids().await, sorted(vec![refused, running]));
    for cleared in [deleted, never_named, gone] {
        assert!(
            report
                .results
                .iter()
                .any(|job| job.id == cleared && job.refusal.is_none()),
            "{cleared} was not reported as cleared: {report:?}"
        );
    }
}

/// The first variant sends nothing anywhere, whatever the rows name at their provider.
#[tokio::test]
async fn clearing_here_only_asks_no_provider() {
    let harness = harness().await;
    harness
        .job(harness.realdebrid, RemoteJobState::Ready, Some("RD-1"))
        .await;
    harness
        .job(harness.torbox, RemoteJobState::AwaitingChoice, Some("TB-1"))
        .await;
    let running = harness
        .job(harness.torbox, RemoteJobState::Preparing, Some("TB-2"))
        .await;

    let report = harness
        .service
        .clear(&RemoteJobClearFilter::default(), false)
        .await
        .expect("clear");

    assert_eq!((report.removed(), report.failed()), (2, 0), "{report:?}");
    assert!(
        harness.discarded().is_empty(),
        "a local clear reached a provider"
    );
    assert_eq!(harness.ids().await, vec![running]);
}

/// The filter is what the person saw: one provider's failed jobs, and nothing of the others.
#[tokio::test]
async fn the_filter_reaches_only_its_own_provider_and_states() {
    let harness = harness().await;
    let failed_here = harness
        .job(harness.realdebrid, RemoteJobState::Failed, None)
        .await;
    let ready_here = harness
        .job(harness.realdebrid, RemoteJobState::Ready, None)
        .await;
    let failed_there = harness
        .job(harness.torbox, RemoteJobState::Failed, None)
        .await;

    let filter = RemoteJobClearFilter {
        provider: Some(" RealDebrid ".to_owned()),
        states: vec![RemoteJobState::Failed],
    };
    let report = harness.service.clear(&filter, false).await.expect("clear");

    assert_eq!(
        report.results.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![failed_here]
    );
    assert_eq!(report.results[0].provider.as_deref(), Some("realdebrid"));
    assert_eq!(harness.ids().await, sorted(vec![ready_here, failed_there]));
}

fn row(account: AccountId, state: RemoteJobState) -> RemoteJob {
    RemoteJob {
        id: RemoteJobId::new(),
        account_id: account,
        plugin_id: ACCEPTING.to_owned(),
        content_key: "key".to_owned(),
        remote_id: None,
        state,
        source_kind: RemoteJobSourceKind::Magnet,
        source_name: None,
        submit_attempts: 0,
        adoption_checked: false,
        package_id: None,
        entries: Vec::new(),
        chosen: Vec::new(),
        progress_permille: None,
        message: None,
        code: None,
        next_poll_at: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        job_state: None,
    }
}

/// A job whose account is gone has no provider: every provider filter passes it by, and a
/// clear without one still reaches it.
#[test]
fn a_job_without_an_account_matches_no_provider_filter() {
    let orphan = row(AccountId::new(), RemoteJobState::Failed);
    let providers = HashMap::new();
    let by_provider = RemoteJobClearFilter {
        provider: Some("realdebrid".to_owned()),
        states: Vec::new(),
    };
    let (targets, skipped) = select(vec![orphan.clone()], &providers, &by_provider);
    assert!(targets.is_empty() && skipped.is_empty());
    let (targets, _) = select(vec![orphan], &providers, &RemoteJobClearFilter::default());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].1, None);
}

/// Every running state is left out, and only those.
#[test]
fn only_the_running_states_are_left_out() {
    let account = AccountId::new();
    let providers = HashMap::from([(account, "realdebrid".to_owned())]);
    let all = [
        RemoteJobState::Submitting,
        RemoteJobState::Preparing,
        RemoteJobState::AwaitingChoice,
        RemoteJobState::Working,
        RemoteJobState::Ready,
        RemoteJobState::Failed,
        RemoteJobState::Discarded,
    ];
    let rows = all.iter().map(|state| row(account, *state)).collect();
    let (targets, skipped) = select(rows, &providers, &RemoteJobClearFilter::default());
    assert_eq!(
        targets.iter().map(|(job, _)| job.state).collect::<Vec<_>>(),
        vec![
            RemoteJobState::AwaitingChoice,
            RemoteJobState::Ready,
            RemoteJobState::Failed,
            RemoteJobState::Discarded,
        ]
    );
    assert_eq!(skipped.len(), 3);
}
