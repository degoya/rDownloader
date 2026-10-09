//! Clearing the list in one go, here only or at the provider too (RD-1200-01).
//!
//! Per row it is exactly what the two single-job requests do: [`RemoteJobService::forget`]
//! removes the row, and with `at_provider` a confirmed [`RemoteJobService::discard`] goes first.
//! What this adds is the selection and the rule that one row's failure is that row's: a provider
//! that refuses, or cannot be reached, keeps its row and says why, and the next row is cleared
//! all the same. The selection is a filter rather than a list of ids, so a request means what the
//! person saw -- "the failed jobs at this provider" -- even when the list moved in between.
//!
//! A job still running at the provider (`submitting`, `preparing`, `working`) is left out and
//! counted: removing its row would orphan a transfer the sweep is still driving, and deleting it
//! at the provider would end one the person may not have looked at yet.

use std::collections::HashMap;

use super::*;

/// A row was cleared but nothing could be said about it at the provider: the plugins did not
/// load, or the database refused a write. The detail goes to the log, not to the answer.
const CLEAR_FAILED: &str = "remote_job.clear_failed";

/// Which rows a clear reaches.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteJobClearFilter {
    /// Only the jobs of accounts at this provider (its slug, case ignored). `None` or blank:
    /// every provider.
    pub provider: Option<String>,
    /// Only jobs in these states. Empty: every state.
    pub states: Vec<RemoteJobState>,
}

/// What became of one row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClearedRemoteJob {
    pub id: RemoteJobId,
    /// The provider of the job's account, or `None` when the account is gone.
    pub provider: Option<String>,
    /// Why the row stayed, or `None` when it went.
    pub refusal: Option<RemoteJobRefused>,
}

/// What a clear did, row by row.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteJobClearReport {
    /// Every row the filter reached that was not left out, in list order.
    pub results: Vec<ClearedRemoteJob>,
    /// The rows the filter reached that were still running and were left alone.
    pub skipped: Vec<ClearedRemoteJob>,
}

impl RemoteJobClearReport {
    /// How many rows went.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.results
            .iter()
            .filter(|job| job.refusal.is_none())
            .count()
    }

    /// How many rows stayed after a refusal.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.results.len() - self.removed()
    }
}

/// The rows a filter reaches, split into the ones to clear and the ones still running.
///
/// Pure, so the selection can be held without a database: `providers` maps an account to its
/// provider slug, and a job whose account is gone has none and matches no provider filter.
pub(crate) fn select(
    jobs: Vec<RemoteJob>,
    providers: &HashMap<AccountId, String>,
    filter: &RemoteJobClearFilter,
) -> (Vec<(RemoteJob, Option<String>)>, Vec<ClearedRemoteJob>) {
    let wanted_provider = filter
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for job in jobs {
        let provider = providers.get(&job.account_id).cloned();
        if let Some(wanted) = wanted_provider
            && !provider
                .as_deref()
                .is_some_and(|provider| provider.eq_ignore_ascii_case(wanted))
        {
            continue;
        }
        if !filter.states.is_empty() && !filter.states.contains(&job.state) {
            continue;
        }
        if job.state.is_polled() {
            skipped.push(ClearedRemoteJob {
                id: job.id,
                provider,
                refusal: None,
            });
        } else {
            targets.push((job, provider));
        }
    }
    (targets, skipped)
}

impl RemoteJobService {
    /// Clears every row `filter` reaches that is not still running; with `at_provider`, each
    /// job is first deleted at its provider.
    ///
    /// `at_provider` is the confirmation [`discard`] asks for: the caller has confirmed it for
    /// the whole selection, and the handler refuses a request that did not say so. A job that
    /// names nothing at the provider -- never submitted, or discarded already -- has nothing
    /// to delete there, and only its row goes.
    ///
    /// [`discard`]: RemoteJobService::discard
    pub async fn clear(
        &self,
        filter: &RemoteJobClearFilter,
        at_provider: bool,
    ) -> anyhow::Result<RemoteJobClearReport> {
        let providers: HashMap<AccountId, String> = self
            .inner
            .database
            .list_accounts()
            .await?
            .into_iter()
            .map(|account| (account.id, account.provider))
            .collect();
        let (targets, skipped) = select(self.jobs().await?, &providers, filter);
        let mut results = Vec::with_capacity(targets.len());
        for (job, provider) in targets {
            let mut refusal = None;
            if at_provider && job.state != RemoteJobState::Discarded && job.remote_id.is_some() {
                refusal = self.discard_for_clear(job.id).await;
            }
            if refusal.is_none()
                && let Err(error) = self.forget(job.id).await
            {
                tracing::warn!(remote_job = %job.id, %error, "a cleared remote job's row stayed");
                refusal = Some(RemoteJobRefused::new(
                    CLEAR_FAILED,
                    "the row could not be removed from this list",
                ));
            }
            results.push(ClearedRemoteJob {
                id: job.id,
                provider,
                refusal,
            });
        }
        Ok(RemoteJobClearReport { results, skipped })
    }

    /// One confirmed discard of a clear, answering only why it did not happen.
    async fn discard_for_clear(&self, id: RemoteJobId) -> Option<RemoteJobRefused> {
        match self.discard(id, true).await {
            Ok(DiscardOutcome::Discarded(_)) => None,
            // Gone since the list was read: there is no row left to keep.
            Ok(DiscardOutcome::Refused(refusal)) if refusal.code == NOT_FOUND => None,
            Ok(DiscardOutcome::Refused(refusal)) => Some(refusal),
            Err(error) => {
                tracing::warn!(remote_job = %id, %error, "a remote job was not deleted at the provider");
                Some(RemoteJobRefused::new(
                    CLEAR_FAILED,
                    "the job could not be deleted at the provider",
                ))
            }
        }
    }
}
