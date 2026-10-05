//! Jobs that run at a provider: the duplicate guard, a named job, a late answer.

use crate::{Database, NewAccount};

/// RD-107-06: the duplicate guard of a job that runs at a provider, at the level it is made.
///
/// `torrents/addMagnet` is not idempotent — it answers with a new id every time — so the only
/// place a second submit can be prevented is *before* the first one: a row carrying the content
/// key, written first, with a unique index behind it. A person pasting the same magnet twice
/// therefore finds the job they already started instead of starting a second one, and so does a
/// restart that replays the same intake.
#[tokio::test]
async fn a_second_remote_job_for_the_same_content_is_refused_rather_than_created() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-jobs.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: Some("secret://realdebrid/client-secret".to_owned()),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let claim = |id: rd_core::RemoteJobId| crate::ClaimRemoteJob {
        id,
        account_id: account.id,
        plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
        content_key: "c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2".to_owned(),
        source_kind: rd_core::RemoteJobSourceKind::Magnet,
        source: b"magnet:?xt=urn:btih:c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2".to_vec(),
        source_name: None,
        package_id: None,
    };

    let first = database
        .claim_remote_job(claim(rd_core::RemoteJobId::new()))
        .await
        .expect("first claim");
    assert_eq!(first.state, rd_core::RemoteJobState::Submitting);
    assert!(first.remote_id.is_none(), "nothing has been submitted yet");

    // The second claim is refused, not merged and not overwritten: a row that silently became
    // a different job would be the duplicate arriving by another door.
    database
        .claim_remote_job(claim(rd_core::RemoteJobId::new()))
        .await
        .expect_err("a second job for the same content on the same account");

    // And the caller can find what it collided with, which is what makes the refusal usable.
    let existing = database
        .remote_job_by_content(account.id, &first.content_key)
        .await
        .expect("lookup")
        .expect("the job that was already there");
    assert_eq!(existing.id, first.id);
}

/// A restart in the middle of polling must not start a second job at the provider.
///
/// Three things together make that true, and this exercises all three: the identifier is
/// written the moment the provider names it, a written identifier ends the submit path for
/// ever, and a row that ended cannot be walked back into a state that would submit again.
#[tokio::test]
async fn a_remote_job_that_was_named_by_the_provider_never_submits_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-restart.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: Some("secret://realdebrid/client-secret".to_owned()),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let id = rd_core::RemoteJobId::new();
    database
        .claim_remote_job(crate::ClaimRemoteJob {
            id,
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: "deadbeef".to_owned(),
            source_kind: rd_core::RemoteJobSourceKind::Magnet,
            source: b"magnet:?xt=urn:btih:deadbeef".to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("claim");

    // The submit went out. Before the answer comes back, the attempt is on the row: that is
    // what tells a restart to ask the provider what it holds rather than to send again.
    let attempted = database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                count_submit_attempt: true,
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("attempt counted");
    assert_eq!(attempted.submit_attempts, 1);
    assert_eq!(attempted.submit_step(), rd_core::SubmitStep::Adopt);

    // The identifier arrives and is written as the very next thing that happens.
    let named = database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT1".to_owned()),
                state: Some(rd_core::RemoteJobState::Preparing),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("named");
    assert_eq!(named.remote_id.as_deref(), Some("RDTORRENT1"));
    assert_eq!(named.submit_step(), rd_core::SubmitStep::Poll);

    // A restart re-reads the row and reaches the same conclusion, which is the whole point of
    // the row being where the state lives.
    let reopened = database.remote_job(id).await.expect("read").expect("job");
    assert_eq!(reopened.submit_step(), rd_core::SubmitStep::Poll);

    // A second identifier is a second job at the provider; overwriting would lose the first.
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT2".to_owned()),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("a second remote identifier");

    // And nothing walks a job back into the state that would submit.
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                state: Some(rd_core::RemoteJobState::Submitting),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("back to submitting");
}

/// A late poll answer must not resurrect a job somebody deleted at the provider.
#[tokio::test]
async fn a_discarded_remote_job_is_not_brought_back_by_a_late_answer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-discard.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let id = rd_core::RemoteJobId::new();
    database
        .claim_remote_job(crate::ClaimRemoteJob {
            id,
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: "feedface".to_owned(),
            source_kind: rd_core::RemoteJobSourceKind::Container,
            source: b"d8:announce".to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("claim");
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT3".to_owned()),
                state: Some(rd_core::RemoteJobState::Discarded),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("discarded");
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                state: Some(rd_core::RemoteJobState::Working),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("a late poll answer");
    // The row stays, so the person can still see what happened to it.
    let job = database.remote_job(id).await.expect("read").expect("job");
    assert_eq!(job.state, rd_core::RemoteJobState::Discarded);
}
