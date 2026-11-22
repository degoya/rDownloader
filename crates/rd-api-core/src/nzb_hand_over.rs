//! An NZB handed to a remote-job provider (RD-191-13, RD-1460-03).
//!
//! Two areas hand NZBs over: the queue (an NZB import of the LinkGrabber, the NZB behind a queued
//! package) and the intake (a hit of an indexer subscription). What has to hold before anything
//! leaves the machine -- the account exists, an installed plugin runs remote jobs on its
//! provider and takes NZB files -- the ceiling a remote job's container keeps, and the
//! submission through `submit_named`, with its duplicate guard, are this one implementation, so
//! the codes cannot drift apart between the places an NZB is handed over from.

use rd_core::{AccountId, RemoteJob};
use rd_plugin_host::extension::RemoteJobSource;

use crate::{
    ApiError, AppState,
    remote_job_service::{MAX_REMOTE_JOB_CONTAINER_BYTES, SubmitOutcome},
};

/// The container format an NZB is handed over as, in `[extension] containers`.
pub const NZB_CONTAINER: &str = "nzb";

/// The job an NZB went to.
#[derive(Debug)]
pub struct NzbHandedOver {
    pub job: RemoteJob,
    /// True when the account already had a job for this NZB. Nothing was written and nothing
    /// was sent -- the duplicate guard answering, not a failure.
    pub already_running: bool,
}

/// Refuses an account an NZB cannot go to, before the NZB is written out or fetched.
///
/// # Errors
///
/// `remote_job.no_account` (`404`) for an account that does not exist,
/// `nzb.remote_job_not_remote_account` (`400`) when no installed plugin runs remote jobs on its
/// provider, `nzb.remote_job_no_nzb` (`400`) when that plugin takes no NZB files.
pub async fn ensure_takes_nzb(state: &AppState, account_id: AccountId) -> Result<(), ApiError> {
    let account = state
        .database
        .get_account(account_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "remote_job.no_account",
                "the account this job would run on does not exist",
            )
        })?;
    let provider = account.provider.to_ascii_lowercase();
    if !state
        .queue
        .remote_jobs
        .providers()
        .await?
        .contains(&provider)
    {
        return Err(ApiError::bad_request(
            "nzb.remote_job_not_remote_account",
            "No installed plugin runs remote jobs on this account's provider",
        ));
    }
    if !state
        .queue
        .remote_jobs
        .providers_accepting(NZB_CONTAINER)
        .await?
        .contains(&provider)
    {
        return Err(ApiError::bad_request(
            "nzb.remote_job_no_nzb",
            "This account's provider does not take NZB files",
        ));
    }
    Ok(())
}

/// Submits an NZB document to one account as a container job named `name`.
///
/// Call [`ensure_takes_nzb`] first: this is the half after the document exists.
///
/// # Errors
///
/// `nzb.remote_job_too_large` (`413`) above the 16 MiB a remote job's container may carry,
/// `remote_job.not_claimed` (`400`) when the plugin takes no such job, and the plugin's own
/// refusal with its code.
pub async fn submit_nzb(
    state: &AppState,
    account_id: AccountId,
    content: Vec<u8>,
    name: Option<String>,
) -> Result<NzbHandedOver, ApiError> {
    if content.len() > MAX_REMOTE_JOB_CONTAINER_BYTES {
        let max_mib = MAX_REMOTE_JOB_CONTAINER_BYTES / (1024 * 1024);
        return Err(ApiError::payload_too_large(
            "nzb.remote_job_too_large",
            format!("The NZB exceeds the {max_mib} MiB a remote job may carry"),
        )
        .with_param("max_mib", max_mib));
    }
    let outcome = state
        .queue
        .remote_jobs
        .submit_named(account_id, RemoteJobSource::Container(content), name)
        .await?;
    match outcome {
        SubmitOutcome::Started(job) => Ok(NzbHandedOver {
            job,
            already_running: false,
        }),
        SubmitOutcome::AlreadyOurs(job) => Ok(NzbHandedOver {
            job,
            already_running: true,
        }),
        SubmitOutcome::NotClaimed => Err(ApiError::bad_request(
            "remote_job.not_claimed",
            "no installed plugin runs a job like this on this account's provider",
        )),
        SubmitOutcome::Refused(refusal) => Err(refusal.into_api_error()),
    }
}
