//! Resolving a download now, without transferring it (RD-1250-05).
//!
//! "Re-resolve" used to drop the resolver pin and leave the rest to the next start, so the size
//! and the name of a file were only known once its download began. This is the resolver part of
//! a worker's attempt on its own: the same resolver chain, the account, proxy and pin the file
//! carries, then the stream-transform plugins where no resolver spoke. Nothing is transferred,
//! no state is changed and no pin is claimed; the next start claims one as always.
//!
//! Which links are resolved ahead is the caller's decision: `rd-api-queue` skips a hoster link
//! without an account (owner, 2026-10-11), since resolving it would run the hoster's free flow.

use anyhow::Result;
use rd_core::{DownloadFile, Failure};
use rd_plugin_api::ResolvedDownload;

use crate::{NetworkClient, SchedulerHandle};

/// What resolving a link now found.
pub enum ResolvedNow {
    /// A resolver or a stream-transform plugin answered for the link.
    Resolved(Box<ResolvedDownload>),
    /// No plugin speaks for the address: a plain direct link, which the caller probes with
    /// this client. A link a stranger's document or page proposed is held to the address rule
    /// its sources were written with (RD-150-03): refused here when it points where it may not
    /// reach, and the client is the guarded one.
    Direct(NetworkClient),
}

impl SchedulerHandle {
    /// Resolves `file` the way its next attempt would, and transfers nothing. The outer error
    /// is the store's; the inner one is the plugin's answer, coded like a worker's failure.
    pub async fn resolve_now(&self, file: &DownloadFile) -> Result<Result<ResolvedNow, Failure>> {
        let pin = self.database.resolver_pin(file.id).await?;
        let resolved = self
            .resolvers
            .resolve(
                file.source.clone(),
                file.account_id,
                file.proxy_profile_id,
                pin.as_ref(),
            )
            .await;
        match resolved {
            Ok(Some(resolved)) => return Ok(Ok(ResolvedNow::Resolved(Box::new(resolved)))),
            Ok(None) => {}
            Err(failure) => return Ok(Err(failure)),
        }
        if self.transforms.is_empty() || !self.transforms.claims(file.source.as_str()).await {
            return self.direct_probe(file).await;
        }
        // The vaulted fragment goes back onto the address for this one call, exactly as the
        // worker restores it (RD-110-38); the restored address never leaves this function.
        let mut url = file.source.clone();
        match self.database.download_secret_fragment(file.id).await {
            Ok(Some(fragment)) => url.set_fragment(Some(&fragment)),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                %error,
                download_id = %file.id,
                "the vaulted link fragment could not be read back"
            ),
        }
        let request = rd_plugin_api::ResolveRequest {
            url,
            client: rd_plugin_api::ClientIdentity {
                account_id: file.account_id,
                proxy_profile_id: file.proxy_profile_id,
                tls_revision: 0,
            },
        };
        match self.transforms.resolve(&request).await {
            None => self.direct_probe(file).await,
            Some(Ok(answer)) => Ok(Ok(ResolvedNow::Resolved(Box::new(answer.download)))),
            Some(Err(failure)) => Ok(Err(failure)),
        }
    }

    /// The client a direct link is probed with, after the address rule its sources keep to.
    async fn direct_probe(&self, file: &DownloadFile) -> Result<Result<ResolvedNow, Failure>> {
        let Some(policy) = self.address_policy_for(file.id).await? else {
            return Ok(Ok(ResolvedNow::Direct(
                self.direct_client(&file.source).await?,
            )));
        };
        if let Err(rd_http::TargetRefusal::Refused(_)) =
            rd_http::check_target(&policy, &rd_http::SystemLookup, &file.source).await
        {
            return Ok(Err(crate::worker::internal_address()));
        }
        Ok(Ok(ResolvedNow::Direct(
            self.guarded_client(&file.source, policy).await?,
        )))
    }
}
