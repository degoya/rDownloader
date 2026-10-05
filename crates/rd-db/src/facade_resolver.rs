//! Database facade for what a download is resolved and replayed with: the consented request
//! template, the refresh budgets, the resets a refresh starts over from and the resolver pin.

use anyhow::Result;
use rd_core::{DownloadFile, DownloadId};

use crate::{Database, commands::DownloadsCommand, parse_id, replay_store, writer};

impl Database {
    /// Consented replay template of a download, if it has one.
    pub async fn request_template(
        &self,
        id: DownloadId,
    ) -> Result<Option<rd_core::RequestTemplate>> {
        replay_store::request_template(&self.readers, id).await
    }

    /// `vault://` reference of a download's request body.
    pub async fn request_body_ref(&self, id: DownloadId) -> Result<Option<String>> {
        replay_store::template_body_ref(&self.readers, id).await
    }

    /// `vault://` reference of a candidate's captured request body.
    pub async fn candidate_body_ref(&self, id: rd_core::CandidateId) -> Result<Option<String>> {
        replay_store::candidate_body_ref(&self.readers, id).await
    }

    /// Replay consent recorded for a candidate.
    pub async fn candidate_replay_consent(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::ReplayConsent>> {
        replay_store::candidate_consent(&self.readers, id).await
    }

    /// Records or withdraws a candidate's replay consent.
    pub async fn set_candidate_replay_consent(
        &self,
        id: rd_core::CandidateId,
        consent: Option<rd_core::ReplayConsent>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::SetCandidateReplayConsent {
                id,
                consent: Box::new(consent),
                reply,
            }
        })
        .await
    }

    /// Atomically reserves one pre-resume replay refresh from the windowed budget.
    ///
    /// Separate from [`Self::claim_resolver_refresh`] on purpose; see `replay_store`.
    pub async fn claim_replay_refresh(&self, id: DownloadId) -> Result<bool> {
        writer::request(&self.writer, |reply| DownloadsCommand::ClaimReplayRefresh {
            id,
            reply,
        })
        .await
    }

    /// Puts a download back to `queued` with everything it produced discarded.
    ///
    /// Unlike `reset_transfer` this is the whole job: the retry budget, the recorded error, the
    /// Usenet segment checkpoints and the package's post-processing steps go with it.
    pub async fn reset_download(&self, id: DownloadId) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::ResetDownload {
            id,
            reply,
        })
        .await
    }

    /// Discards a download's partial state so a refreshed URL starts from zero.
    pub async fn reset_transfer(&self, id: DownloadId) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::ResetTransfer {
            id,
            reply,
        })
        .await
    }

    /// Atomically reserves the single resolver refresh allowed after HTTP 401/403.
    pub async fn claim_resolver_refresh(&self, id: DownloadId) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::ClaimResolverRefresh { id, reply }
        })
        .await
    }

    /// Returns the exact resolver version already assigned to a download.
    pub async fn resolver_pin(&self, id: DownloadId) -> Result<Option<rd_core::ResolverPin>> {
        use sqlx::Row;

        let row = sqlx::query(
            "SELECT plugin_id, plugin_version FROM download_resolver_pins WHERE download_id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.readers)
        .await?;
        row.map(|row| {
            Ok(rd_core::ResolverPin {
                plugin_id: parse_id(row.get::<String, _>("plugin_id").as_str())?,
                version: row.get("plugin_version"),
            })
        })
        .transpose()
    }

    /// Atomically writes the first resolver pin and returns the winner of any race.
    pub async fn claim_resolver_pin(
        &self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<rd_core::ResolverPin> {
        writer::request(&self.writer, |reply| DownloadsCommand::ClaimResolverPin {
            id,
            pin,
            reply,
        })
        .await
    }

    /// Points a download that is not running at one exact resolver version (RD-140-02).
    ///
    /// Unlike [`Self::claim_resolver_pin`] this replaces a pin the download already has: it is
    /// how a download is started "with the version under test". Refused while the download is
    /// running, because a pin is what keeps a running job on one version.
    pub async fn pin_download_resolver(
        &self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::PinDownloadResolver { id, pin, reply }
        })
        .await
    }

    /// Drops resolver pins that name a version this build can no longer provide.
    ///
    /// Returns how many jobs were freed. See the writer implementation for why an
    /// unsatisfiable pin is worse than no pin at all, and what happens to a download whose
    /// pinned version was withdrawn.
    pub async fn clear_unsatisfiable_resolver_pins(
        &self,
        available: Vec<(String, String)>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::ClearUnsatisfiableResolverPins { available, reply }
        })
        .await
    }
}
