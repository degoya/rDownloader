//! The durable half of the remote-job state machine: what survives a restart, and what a
//! restart must not be able to create twice (RD-120-01).
//!
//! The guarantee these exist for is one sentence: **a provider whose create call is not
//! idempotent must not end up with two jobs for one thing, whatever happens to this process.**
//! Nothing a plugin does can promise that -- a guest is instantiated fresh for every call and
//! remembers nothing -- so the promise is made here, by the unique index on
//! `(account_id, content_key)` that migration 0065 declares, and by the fact that the row is
//! written *before* the provider is asked for anything.
//!
//! The plugin's half -- that the key is derived from the source alone, and so is the same
//! before and after a crash -- is driven by
//! `crates/rd-plugin-ext/tests/torbox_remote_job_contract.rs`.

use chrono::{Duration, Utc};
use rd_core::{AccountId, RemoteJobSourceKind, RemoteJobState};
use rd_db::{AdvanceRemoteJob, ClaimRemoteJob, Database, NewAccount};
use tempfile::TempDir;

/// The key `plugins/torbox-jobs/` derives for a magnet: the kind, then the info hash.
const CONTENT_KEY: &str = "torrent:da39a3ee5e6b4b0d3255bfef95601890afd80709";
const PLUGIN_ID: &str = "019d0000-0000-7000-8000-00000000011e";
const MAGNET: &str = "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709";

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("remote-jobs.sqlite"))
        .await
        .expect("database")
}

async fn account(database: &Database) -> AccountId {
    database
        .create_account(NewAccount {
            provider: "torbox".to_owned(),
            label: "TorBox".to_owned(),
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

fn claim(account_id: AccountId, content_key: &str) -> ClaimRemoteJob {
    ClaimRemoteJob {
        id: rd_core::RemoteJobId::new(),
        account_id,
        plugin_id: PLUGIN_ID.to_owned(),
        content_key: content_key.to_owned(),
        source_kind: RemoteJobSourceKind::Magnet,
        source: MAGNET.as_bytes().to_vec(),
        package_id: None,
    }
}

/// The claim comes first, and it comes with nothing from the provider in it. That ordering is
/// the whole guarantee: between writing this row and hearing back from TorBox there is a
/// window, and the row is what makes the window survivable.
#[tokio::test]
async fn a_claim_is_written_before_the_provider_is_asked_for_anything() {
    let directory = TempDir::new().expect("temp dir");
    let database = database(&directory).await;
    let account_id = account(&database).await;

    let job = database
        .claim_remote_job(claim(account_id, CONTENT_KEY))
        .await
        .expect("claimed");
    assert_eq!(job.content_key, CONTENT_KEY);
    assert_eq!(job.remote_id, None, "nothing has been created yet");
    assert_eq!(job.submit_attempts, 0);
    assert!(!job.adoption_checked);
    assert_eq!(job.state, RemoteJobState::Submitting);
}

/// The duplicate guard, and the reason this table exists at all.
///
/// A second claim for the same content on the same account is refused, so there is never a
/// second row to drive a second create call. The same content on a *different* account is a
/// different job, because it runs on a different plan and produces different addresses.
#[tokio::test]
async fn one_account_cannot_hold_two_jobs_for_one_content_key() {
    let directory = TempDir::new().expect("temp dir");
    let database = database(&directory).await;
    let account_id = account(&database).await;

    let first = database
        .claim_remote_job(claim(account_id, CONTENT_KEY))
        .await
        .expect("claimed");
    database
        .claim_remote_job(claim(account_id, CONTENT_KEY))
        .await
        .expect_err("a second row for the same content is refused");
    assert_eq!(
        database
            .remote_jobs(account_id)
            .await
            .expect("listed")
            .len(),
        1
    );

    // The existing row is what the caller is expected to find and show instead.
    let found = database
        .remote_job_by_content(account_id, CONTENT_KEY)
        .await
        .expect("looked up")
        .expect("the row that already exists");
    assert_eq!(found.id, first.id);

    // TorBox's three kinds are three keys, so one release submitted as a magnet and as an NZB
    // is two jobs and not a collision.
    database
        .claim_remote_job(claim(account_id, "usenet:0123456789abcdef0123456789abcdef"))
        .await
        .expect("a different kind is a different job");

    let other = account(&database).await;
    database
        .claim_remote_job(claim(other, CONTENT_KEY))
        .await
        .expect("another account's job is its own");
}

/// A restart in the worst place: the row is written, the create call went out, and the answer
/// never arrived. Afterwards the row is still there, still names no job at the provider, and
/// still remembers the source -- so the sweep can ask TorBox what it already holds instead of
/// asking it to create a second one.
#[tokio::test]
async fn a_restart_between_the_claim_and_the_answer_leaves_a_row_that_adopts() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("remote-jobs.sqlite");
    let claimed = {
        let database = Database::open(path.clone()).await.expect("database");
        let account_id = account(&database).await;
        let job = database
            .claim_remote_job(claim(account_id, CONTENT_KEY))
            .await
            .expect("claimed");
        // The attempt is counted before the answer comes back, because a count written
        // afterwards is a count a crash loses -- and the difference between "asked once" and
        // "never asked" is a duplicate in somebody's account.
        database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    count_submit_attempt: true,
                    ..AdvanceRemoteJob::default()
                },
            )
            .await
            .expect("counted")
    };
    assert_eq!(claimed.submit_attempts, 1);
    assert_eq!(claimed.remote_id, None);

    // The restart. A new handle on the same file, nothing carried over in memory.
    let database = Database::open(path).await.expect("database reopened");
    let job = database
        .remote_job(claimed.id)
        .await
        .expect("read")
        .expect("the row survived");
    assert_eq!(job.content_key, CONTENT_KEY);
    assert_eq!(job.submit_attempts, 1, "the attempt is remembered");
    assert_eq!(job.remote_id, None, "and it is known to have no answer");
    assert!(!job.adoption_checked, "the window has not been looked into");
    // The source is kept so a restart offers the very same bytes rather than asking somebody
    // to paste a magnet a second time.
    assert_eq!(
        database
            .remote_job_source(job.id)
            .await
            .expect("read")
            .as_deref(),
        Some(MAGNET.as_bytes())
    );

    // The adoption found the orphan: the identifier is written and the window is closed.
    let adopted = database
        .advance_remote_job(
            job.id,
            AdvanceRemoteJob {
                state: Some(RemoteJobState::Working),
                remote_id: Some("4711".to_owned()),
                job_state: Some(Some("torrent".to_owned())),
                adoption_checked: true,
                ..AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("adopted");
    assert_eq!(adopted.remote_id.as_deref(), Some("4711"));
    assert!(adopted.adoption_checked);
    assert_eq!(
        adopted.submit_attempts, 1,
        "adopting is not creating, so nothing was counted"
    );
}

/// What a plugin needs handed back to it, and the schedule the host owns.
///
/// `job_state` is the plugin's own bookkeeping -- for TorBox, which of its three kinds the job
/// is -- stored verbatim and returned unchanged. Without it a poll after a restart would not
/// know which of three sets of endpoints to ask.
#[tokio::test]
async fn the_plugins_bookkeeping_and_the_next_poll_both_survive_a_restart() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("remote-jobs.sqlite");
    let due_at = Utc::now() - Duration::seconds(5);
    let id = {
        let database = Database::open(path.clone()).await.expect("database");
        let account_id = account(&database).await;
        let job = database
            .claim_remote_job(claim(account_id, CONTENT_KEY))
            .await
            .expect("claimed");
        database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Working),
                    remote_id: Some("4711".to_owned()),
                    job_state: Some(Some("usenet".to_owned())),
                    progress_permille: Some(Some(425)),
                    next_poll_at: Some(Some(due_at)),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await
            .expect("advanced");
        job.id
    };

    let database = Database::open(path).await.expect("database reopened");
    let job = database
        .remote_job(id)
        .await
        .expect("read")
        .expect("the row survived");
    assert_eq!(job.job_state.as_deref(), Some("usenet"));
    assert_eq!(job.progress_permille, Some(425));
    assert_eq!(job.state, RemoteJobState::Working);
    // And the sweep picks it up again, which is what makes a job started before a restart
    // carry on afterwards instead of waiting for somebody to notice.
    let due = database.due_remote_jobs(Utc::now()).await.expect("due");
    assert!(due.iter().any(|row| row.id == id), "{due:?}");
}
