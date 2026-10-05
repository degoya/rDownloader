//! Database facade methods for the traffic per Usenet server and its quota (RD-1100-05).

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};

use crate::{
    Database,
    commands::NetworkCommand,
    usenet_store::UsenetQuotaInput,
    usenet_traffic_store::{self, UsenetQuotaReached, UsenetServerTraffic},
    writer,
};

impl Database {
    /// Adds one flush of counted bytes per server, in one transaction, and names the servers
    /// whose quota it used up.
    pub async fn record_usenet_traffic(
        &self,
        counts: Vec<(rd_core::UsenetServerId, u64)>,
    ) -> Result<Vec<UsenetQuotaReached>> {
        self.record_usenet_traffic_at(counts, Utc::now()).await
    }

    /// [`Self::record_usenet_traffic`] on the day `now` falls on.
    pub async fn record_usenet_traffic_at(
        &self,
        counts: Vec<(rd_core::UsenetServerId, u64)>,
        now: DateTime<Utc>,
    ) -> Result<Vec<UsenetQuotaReached>> {
        writer::request(&self.writer, |reply| NetworkCommand::RecordUsenetTraffic {
            counts,
            now,
            reply,
        })
        .await
    }

    /// Every configured server with its traffic as of `today`, in priority order.
    pub async fn list_usenet_server_traffic(
        &self,
        today: NaiveDate,
    ) -> Result<Vec<UsenetServerTraffic>> {
        usenet_traffic_store::list(&self.readers, today).await
    }

    /// Sets, changes or removes one server's quota.
    pub async fn set_usenet_quota(
        &self,
        id: rd_core::UsenetServerId,
        input: UsenetQuotaInput,
    ) -> Result<rd_core::UsenetServer> {
        writer::request(&self.writer, |reply| NetworkCommand::SetUsenetQuota {
            id,
            input,
            reply,
        })
        .await
    }
}
