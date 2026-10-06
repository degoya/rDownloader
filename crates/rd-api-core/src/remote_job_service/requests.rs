//! What the interface asks of a remote job: submit one, choose its files, list, discard and
//! forget them.

use super::*;

impl RemoteJobService {
    /// Hands a source to an account: the entry point of every remote job.
    ///
    /// Everything before the row is written happens without a request. The plugin is asked
    /// whether it takes the source and what key it is known by, both answered locally; the
    /// key is looked up and then claimed under the unique index; and only a row that was
    /// actually written is ever offered to the sweep, which is the only place the plugin's
    /// `submit` is called. A second paste of the same magnet therefore ends in
    /// [`SubmitOutcome::AlreadyOurs`] before anything has left the machine.
    pub async fn submit(
        &self,
        account_id: AccountId,
        source: RemoteJobSource,
    ) -> anyhow::Result<SubmitOutcome> {
        self.submit_named(account_id, source, None).await
    }

    /// [`Self::submit`], with the name the source was handed in under -- a container's file
    /// name. The job's LinkGrabber package is named after it when the job finishes.
    ///
    /// Without one, a magnet is listed under its `dn` and an address under its last path
    /// segment (RD-1120-02), so the list of remote jobs names what each row is.
    pub async fn submit_named(
        &self,
        account_id: AccountId,
        source: RemoteJobSource,
        source_name: Option<String>,
    ) -> anyhow::Result<SubmitOutcome> {
        let accounts = self.inner.database.list_accounts().await?;
        let Some(account) = accounts
            .into_iter()
            .find(|account| account.id == account_id)
        else {
            return Ok(SubmitOutcome::Refused(RemoteJobRefused::new(
                NO_ACCOUNT,
                "the account this job would run on does not exist",
            )));
        };
        let runners = self.runners().await?;
        let (plugin_id, content_key) = match runners.identify(&account.provider, &source).await {
            StartOutcome::NotClaimed => return Ok(SubmitOutcome::NotClaimed),
            StartOutcome::Refused(refusal) => return Ok(SubmitOutcome::Refused(refusal.into())),
            StartOutcome::Identified {
                plugin_id,
                content_key,
            } => (plugin_id, content_key),
        };
        if let Some(existing) = self
            .inner
            .database
            .remote_job_by_content(account_id, &content_key)
            .await?
        {
            return Ok(SubmitOutcome::AlreadyOurs(existing));
        }
        let source_name = source_name.or_else(|| super::naming::implied_name(&source));
        let (source_kind, bytes) = match source {
            RemoteJobSource::Magnet(address) => (RemoteJobSourceKind::Magnet, address.into_bytes()),
            RemoteJobSource::Container(bytes) => (RemoteJobSourceKind::Container, bytes),
            RemoteJobSource::Address(address) => {
                (RemoteJobSourceKind::Address, address.into_bytes())
            }
        };
        let claim = ClaimRemoteJob {
            id: RemoteJobId::new(),
            account_id,
            plugin_id,
            content_key: content_key.clone(),
            source_kind,
            source: bytes,
            source_name,
            package_id: None,
        };
        match self.inner.database.claim_remote_job(claim).await {
            Ok(job) => Ok(SubmitOutcome::Started(job)),
            Err(error) => {
                // Two submits in the same moment: the read above saw nothing and the unique
                // index refused the second insert. That is the guard doing its job, not a
                // failure, so the row the other submit wrote is what this one answers with.
                match self
                    .inner
                    .database
                    .remote_job_by_content(account_id, &content_key)
                    .await?
                {
                    Some(existing) => Ok(SubmitOutcome::AlreadyOurs(existing)),
                    None => Err(error),
                }
            }
        }
    }

    /// Answers the question a job in `awaiting_choice` asked.
    ///
    /// The choice is kept to what the job offered, an empty one never reaches the guest, and
    /// a refusal leaves the row waiting: the question stays open until it is answered.
    pub async fn choose(
        &self,
        id: RemoteJobId,
        requested: &[u32],
    ) -> anyhow::Result<ChoiceOutcome> {
        let Some(job) = self.inner.database.remote_job(id).await? else {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                NOT_FOUND,
                "there is no such remote job",
            )));
        };
        if job.state != RemoteJobState::AwaitingChoice {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                NOT_AWAITING_CHOICE,
                "this remote job is not waiting for a choice",
            )));
        }
        let chosen = job.accept_choice(requested);
        if chosen.is_empty() {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                EMPTY_CHOICE,
                "a selection has to name at least one entry the job offered",
            )));
        }
        let Some(handle) = handle_of(&job) else {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                MISSING_REMOTE_ID,
                "this remote job names no job at the provider",
            )));
        };
        let runners = self.runners().await?;
        if let Err(refusal) = runners
            .choose(&job.plugin_id, job.account_id, &handle, &chosen)
            .await
        {
            return Ok(ChoiceOutcome::Refused(refusal.into()));
        }
        let job = self
            .inner
            .database
            .advance_remote_job(
                id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Working),
                    chosen: Some(chosen),
                    code: Some(None),
                    message: Some(None),
                    // Polled again at once: the person is waiting to see it move.
                    next_poll_at: Some(Some(Utc::now())),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(ChoiceOutcome::Chosen(Box::new(job)))
    }

    /// Every remote job this installation knows about, newest first.
    pub async fn jobs(&self) -> anyhow::Result<Vec<RemoteJob>> {
        self.inner.database.all_remote_jobs().await
    }

    /// Deletes the job **at the provider**, on one explicit confirmed request.
    ///
    /// This is the only path in the application that reaches a plugin's `discard`, which is
    /// what ADR 0003 asks for: not from removing the local package, not from a failed job, not
    /// from a cleanup sweep, and not from a request that merely arrived at this endpoint. The
    /// confirmation is a value the caller has to carry, so "was it confirmed" is answerable
    /// from the request rather than from a client's good intentions.
    ///
    /// What it did is recorded rather than erased. The row stays, in `discarded`, still naming
    /// the job the provider knew — an account that lost a torrent can be shown which request
    /// removed it and when. Forgetting the row is a separate act ([`forget`]).
    ///
    /// [`forget`]: RemoteJobService::forget
    pub async fn discard(
        &self,
        id: RemoteJobId,
        confirmed: bool,
    ) -> anyhow::Result<DiscardOutcome> {
        if !confirmed {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                NOT_CONFIRMED,
                "deleting a job at the provider has to be confirmed explicitly",
            )));
        }
        let Some(job) = self.inner.database.remote_job(id).await? else {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                NOT_FOUND,
                "there is no such remote job",
            )));
        };
        if job.state == RemoteJobState::Discarded {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                ALREADY_DISCARDED,
                "this remote job has already been removed at the provider",
            )));
        }
        let Some(handle) = handle_of(&job) else {
            // Nothing was ever created there, so there is nothing to delete. Saying so is more
            // use than sending a request that would name no job: the row itself can be removed
            // from the list, which touches no provider.
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                MISSING_REMOTE_ID,
                "this remote job names no job at the provider",
            )));
        };
        let runners = self.runners().await?;
        if let Err(refusal) = runners
            .discard(&job.plugin_id, job.account_id, &handle)
            .await
        {
            return Ok(DiscardOutcome::Refused(refusal.into()));
        }
        tracing::info!(
            remote_job = %job.id,
            account = %job.account_id,
            plugin = %job.plugin_id,
            remote_id = %handle.remote_id,
            "a confirmed request removed a job at the provider"
        );
        let job = self
            .inner
            .database
            .advance_remote_job(
                id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Discarded),
                    code: Some(Some(DISCARDED.to_owned())),
                    message: Some(Some(clip("removed at the provider on a confirmed request"))),
                    next_poll_at: Some(None),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(DiscardOutcome::Discarded(Box::new(job)))
    }

    /// Removes the row from this installation's list, and nothing else.
    ///
    /// The counterpart of [`discard`], and the reason the two are separate calls: what the
    /// provider holds is untouched, so a person clearing a finished job out of a list cannot
    /// accidentally delete a torrent in their account. Answers whether a row was there.
    ///
    /// [`discard`]: RemoteJobService::discard
    pub async fn forget(&self, id: RemoteJobId) -> anyhow::Result<bool> {
        self.inner.database.delete_remote_job(id).await
    }
}
