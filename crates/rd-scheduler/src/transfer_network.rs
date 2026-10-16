//! The network a file's own HTTP transfer goes out on, for a runner that fetches over HTTP
//! itself (RD-1320-05).

use anyhow::Result;
use rd_core::DownloadFile;

use crate::{NetworkClient, SchedulerHandle};

/// Builds the client the file's HTTP transfer would use: its account, proxy, authentication
/// profile, custom CA and address rule.
///
/// For a runner whose engine would otherwise fetch an address with a client of its own — the
/// `.torrent` behind a torrent row, which librqbit fetches without any of these. Built on
/// demand, so a runner that never asks costs nothing.
#[derive(Clone)]
pub struct TransferNetwork {
    scheduler: SchedulerHandle,
    file: Box<DownloadFile>,
    address_policy: Option<rd_http::AddressPolicy>,
}

impl TransferNetwork {
    /// The pooled client and its per-request credential headers.
    pub async fn client(&self) -> Result<NetworkClient> {
        crate::worker::build_replay_client(
            &self.scheduler,
            &self.file,
            None,
            self.address_policy.clone(),
        )
        .await
    }
}

impl SchedulerHandle {
    /// The network `file`'s own transfer goes out on, held to `address_policy` when its address
    /// was proposed by a stranger (RD-150-03).
    #[must_use]
    pub fn transfer_network(
        &self,
        file: &DownloadFile,
        address_policy: Option<rd_http::AddressPolicy>,
    ) -> TransferNetwork {
        TransferNetwork {
            scheduler: self.clone(),
            file: Box::new(file.clone()),
            address_policy,
        }
    }
}
