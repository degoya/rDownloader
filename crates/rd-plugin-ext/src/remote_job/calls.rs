//! The calls a row makes on its plugin: identify, submit, adopt, poll, choose and discard, each
//! routed by the provider slug (a new job) or by the plugin id the row names (an existing one).
//!
//! Split out of `remote_job.rs` (PLUG-21).

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_host::extension::{RemoteJobHandle, RemoteJobSource};

use super::{CHOICE_NOT_KEPT, JobRefusal, NO_PLUGIN, PollOutcome, RemoteJobRunners, StartOutcome};

impl RemoteJobRunners {
    /// Whether the plugin for `provider_slug` takes this source, and the key it is known by.
    ///
    /// Reaches nothing. Both calls are answered from the source alone, which is what lets the
    /// caller write the row -- and refuse a duplicate -- before any request goes out.
    pub async fn identify(&self, provider_slug: &str, source: &RemoteJobSource) -> StartOutcome {
        let Some(runner) = self
            .by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
        else {
            return StartOutcome::Refused(JobRefusal::permanent(
                NO_PLUGIN,
                format!("no installed plugin runs remote jobs on {provider_slug}"),
            ));
        };
        match runner.plugin.claims(source).await {
            Ok(true) => {}
            Ok(false) => return StartOutcome::NotClaimed,
            Err(error) => return StartOutcome::Refused(Self::trapped(runner, &error)),
        }
        match runner.plugin.identify(source).await {
            Ok(Ok(content_key)) => StartOutcome::Identified {
                plugin_id: runner.info.plugin_id.clone(),
                content_key,
            },
            Ok(Err(refusal)) => StartOutcome::Refused(refusal.into()),
            Err(error) => StartOutcome::Refused(Self::trapped(runner, &error)),
        }
    }

    /// Hands the source to the provider through the plugin the row names.
    ///
    /// Assumed not idempotent, and nothing here retries: the attempt ceiling, the adoption
    /// check between attempts and the unique row are all the caller's.
    pub async fn submit(
        &self,
        plugin_id: &str,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<RemoteJobHandle, JobRefusal> {
        self.submit_named(plugin_id, account, source, content_key, None)
            .await
    }

    /// [`Self::submit`], with the name the source was added under -- a container's file name,
    /// which a provider that names its job after the upload needs (`job-context`).
    pub async fn submit_named(
        &self,
        plugin_id: &str,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
        source_name: Option<&str>,
    ) -> Result<RemoteJobHandle, JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(
            runner,
            runner
                .plugin
                .submit_named(account, source, content_key, source_name)
                .await,
        )
    }

    /// The job the provider already holds for `content_key`, if any.
    pub async fn adopt(
        &self,
        plugin_id: &str,
        account: AccountId,
        content_key: &str,
    ) -> Result<Option<RemoteJobHandle>, JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.adopt(account, content_key).await)
    }

    /// Where the job stands now.
    pub async fn poll(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> PollOutcome {
        let Some(runner) = self.runner(plugin_id) else {
            return PollOutcome::Refused(JobRefusal::no_plugin(plugin_id));
        };
        match runner.plugin.poll(account, handle).await {
            Ok(Ok(progress)) => Self::accept(runner, progress),
            Ok(Err(refusal)) => PollOutcome::Refused(refusal.into()),
            Err(error) => PollOutcome::Refused(Self::trapped(runner, &error)),
        }
    }

    /// Where a job stands whose question has already been answered (RD-120-35).
    ///
    /// `choose` returns nothing and nothing the host keeps about the answer reaches the guest
    /// again, so `awaiting-choice` is only honest from a provider that keeps the answer itself
    /// -- Real-Debrid leaves `waiting_files_selection` the moment `selectFiles` succeeds. A
    /// guest that asks again after an answer is one whose provider did not keep it, and
    /// passing the question on would put the row back where it stood before the answer, to
    /// be answered again, for ever. So it ends the job under its own code instead: a row
    /// never moves from an answered question back to an open one.
    pub async fn poll_answered(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> PollOutcome {
        match self.poll(plugin_id, account, handle).await {
            PollOutcome::AwaitingChoice(_) => PollOutcome::Refused(JobRefusal::permanent(
                CHOICE_NOT_KEPT,
                format!(
                    "{} asked for a choice again after it was answered",
                    self.plugin_name(plugin_id)
                        .unwrap_or("the remote-job plugin")
                ),
            )),
            outcome => outcome,
        }
    }

    /// Answers the question the job asked.
    ///
    /// An empty choice never reaches the guest, but that is not decided here: the service
    /// refuses one before it calls this, and the host wrapper refuses one again before it
    /// instantiates anything. This adapter only carries the call through.
    pub async fn choose(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<(), JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.choose(account, handle, chosen).await)
    }

    /// Removes the job at the provider. Reached from one confirmed request and nothing else.
    pub async fn discard(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<(), JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.discard(account, handle).await)
    }
}
