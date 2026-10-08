//! The worker of the pick board: resolves the chosen entries of one page, one after the other.
//! Split out of `picks.rs` (RD-1190-17) to keep that file short; nothing here changed.

use async_trait::async_trait;
use rd_siterules::{CrawlGroup, RunError};

use super::{EntryOutcome, PickJob};
use crate::siterules::SiteRules;

/// Hands one resolved entry to the LinkGrabber. Implemented where the collector is.
#[async_trait]
pub trait PickDelivery: Send + Sync {
    /// Adds the entry's links as one package; how many were kept, or the stable code of the
    /// refusal.
    async fn deliver(&self, job: &PickJob, group: CrawlGroup) -> Result<u32, String>;
}

/// What a refusal of the second stage makes of the entry.
fn outcome_of(error: &RunError) -> EntryOutcome {
    match error {
        // Nobody answered, somebody declined, or no broker could ask: the entry is untouched
        // and can be picked again.
        RunError::CaptchaFailed { .. } | RunError::LimitTime(_) => {
            EntryOutcome::Pending(error.code().to_owned())
        }
        _ => EntryOutcome::Failed(error.code().to_owned()),
    }
}

impl SiteRules {
    /// Resolves the queued entries of a page's `round` one after the other until none is left
    /// or the page is stopped. Started once per round by whoever queued the entries, with the
    /// round [`PickBoard::queue`] named.
    pub async fn work_picks(&self, id: &str, round: u64, delivery: &dyn PickDelivery) {
        let Some(stopper) = self.picks().stopper(id) else {
            return;
        };
        loop {
            // Waiting before the entry is taken: a stop between the two is not missed.
            let stopped = stopper.notified();
            tokio::pin!(stopped);
            let Some(job) = self.picks().next(id, round) else {
                return;
            };
            let runner = self.runner();
            let resolved = tokio::select! {
                result = runner.resolve(&job.rule, &job.address, &job.list, job.index) => result,
                () = &mut stopped => {
                    // `cancel` already put the entry back; nothing is delivered.
                    tracing::info!(page = %job.page, "resolving a picked entry was stopped");
                    return;
                }
            };
            let outcome = match resolved {
                Ok(group) => match delivery.deliver(&job, group).await {
                    Ok(links) => EntryOutcome::Done(links),
                    Err(code) => EntryOutcome::Failed(code),
                },
                Err(error) => {
                    tracing::info!(
                        rule = %job.rule.name,
                        code = error.code(),
                        "a picked entry could not be resolved"
                    );
                    outcome_of(&error)
                }
            };
            self.picks().finish(id, job.index, outcome);
        }
    }
}
