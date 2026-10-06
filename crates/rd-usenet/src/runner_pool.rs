//! The NNTP pool a Usenet runner keeps between files, rebuilt only when the servers, their
//! trust roots or the per-file cap change.

use anyhow::Result;
use rd_core::{Failure, FailureKind};

use super::UsenetRunner;
use crate::NntpPool;

/// How long a download waits before it asks again when every enabled server is paused by its
/// quota (RD-1100-05). A wait for a limit, not an attempt: a quota is raised or reset by a
/// person or on its reset day, and the queue should notice within minutes, not hours.
const QUOTA_RETRY_SECONDS: u64 = 15 * 60;

/// A pool and what it was built from; it is replaced when either changes.
pub(super) struct CachedPool {
    fingerprint: String,
    cap: Option<usize>,
    pool: NntpPool,
}

impl UsenetRunner {
    /// The pool for the current settings, built only if there is not one already.
    pub(crate) async fn pool(&self, cap: Option<usize>) -> Result<NntpPool> {
        let (custom_ca_pem, tls_revision) = {
            let defaults = self.network.read().await;
            (defaults.custom_ca_pem.clone(), defaults.tls_revision)
        };
        // The TLS revision belongs in the key for the same reason the server records do: a pool
        // that outlives a single file would otherwise keep handing out connections built on the
        // trust roots the operator has just replaced.
        let fingerprint = format!(
            "{}|tls{tls_revision}",
            crate::connection_fingerprint(&self.database).await?
        );
        let mut cached = self.pool.lock().await;
        if let Some(current) = cached.as_ref()
            && current.fingerprint == fingerprint
            && current.cap == cap
        {
            return Ok(current.pool.clone());
        }
        let ordered =
            crate::servers_by_quota(&self.database, &self.secrets, &custom_ca_pem).await?;
        if ordered.servers.is_empty() && ordered.paused > 0 {
            return Err(Failure::coded(
                FailureKind::RateLimited {
                    retry_after_seconds: Some(QUOTA_RETRY_SECONDS),
                },
                "usenet.quota_reached",
                "Every enabled Usenet server is paused because its quota is used up",
            )
            .with_param("servers", ordered.paused)
            .into());
        }
        let servers = ordered
            .servers
            .into_iter()
            .map(|(id, config)| (config, Some(self.traffic.counter(id))))
            .collect();
        let pool = NntpPool::metered(servers, cap)?;
        *cached = Some(CachedPool {
            fingerprint,
            cap,
            pool: pool.clone(),
        });
        Ok(pool)
    }
}
