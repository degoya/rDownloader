//! The `sink` and `net` host functions a transfer backend runs against.
//!
//! Both live here rather than beside their data because they are the *boundary*: everything
//! above is the host's own code, everything below is a guest asking for something. Every
//! function starts by finding the transfer state, and a store without one — a resolver's —
//! simply has nothing to answer with.

use std::time::Instant;

use rd_core::{Failure, FailureKind};

use super::{
    connection::{self, HostConnection},
    rdownloader::plugin::{net, sink},
    state::TransferState,
};
use crate::{component::to_wit_failure, domain_allowed, runtime::PluginStoreState};

impl PluginStoreState {
    fn transfer_mut(&mut self) -> Result<&mut TransferState, Failure> {
        self.transfer.as_mut().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.not_a_transfer",
                "This plugin is not running a transfer",
            )
        })
    }
}

impl sink::Host for PluginStoreState {
    async fn write_at(
        &mut self,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), super::rdownloader::plugin::types::Failure> {
        let written = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let started = Instant::now();
        let transfer = self.transfer_mut().map_err(to_wit_failure)?;
        let part = transfer.target.part.clone();
        let result = part.write_at(offset, bytes).await;
        let transfer = self.transfer_mut().map_err(to_wit_failure)?;
        if let Err(error) = result {
            // Remembered so a backend that ignores the error cannot report a completion the
            // disk never received.
            transfer.note_sink_failure();
            tracing::warn!(error = %error, "plugin transfer sink write failed");
            return Err(to_wit_failure(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "plugin.sink_write_failed",
                "The transfer could not be written to disk",
            )));
        }
        // The host, not the guest, decides how far the file has got: `committed` is the high
        // water mark of what actually reached the disk, which is what a resume must trust.
        transfer.target.committed = transfer
            .target
            .committed
            .max(offset.saturating_add(written));
        self.credit_host_time(started.elapsed());
        Ok(())
    }

    async fn committed(&mut self) -> u64 {
        self.transfer.as_ref().map_or(0, TransferState::committed)
    }

    async fn sync(&mut self) -> Result<(), super::rdownloader::plugin::types::Failure> {
        let started = Instant::now();
        let part = self
            .transfer_mut()
            .map_err(to_wit_failure)?
            .target
            .part
            .clone();
        let result = part.sync_data().await;
        self.credit_host_time(started.elapsed());
        result.map_err(|error| {
            tracing::warn!(error = %error, "plugin transfer sink sync failed");
            to_wit_failure(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "plugin.sink_sync_failed",
                "The transfer could not be flushed to disk",
            ))
        })
    }

    async fn progress(&mut self, committed: u64, total: Option<u64>) {
        if let Some(transfer) = self.transfer.as_mut() {
            if total.is_some() {
                transfer.target.total = total;
            }
            (transfer.progress)(transfer.target.committed.max(committed), total);
        }
    }

    async fn should_stop(&mut self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|transfer| transfer.cancellation.is_cancelled())
    }
}

impl net::Host for PluginStoreState {
    async fn connect(
        &mut self,
        host: String,
        port: u16,
        tls: bool,
    ) -> Result<u32, super::rdownloader::plugin::types::Failure> {
        let started = Instant::now();
        let (config, allow_local) = {
            let transfer = self.transfer_mut().map_err(to_wit_failure)?;
            let Some(stream) = transfer.stream.clone() else {
                return Err(to_wit_failure(refused("net_stream")));
            };
            if !connection::within_connection_limit(transfer.open_connections) {
                return Err(to_wit_failure(Failure::coded(
                    FailureKind::Permanent,
                    "plugin.net_too_many_connections",
                    "The plugin opened more connections than one transfer may hold",
                )));
            }
            if !host_allowed(&host, &stream.hosts) || !stream.ports.contains(&port) {
                return Err(to_wit_failure(Failure::coded(
                    FailureKind::Permanent,
                    "plugin.net_target_not_allowed",
                    "The connection target is outside the plugin's declared hosts and ports",
                )));
            }
            (transfer.tls.clone(), transfer.allow_local)
        };
        if !allow_local {
            refuse_local_targets(&host, port)
                .await
                .map_err(to_wit_failure)?;
        }
        let opened = HostConnection::open(&host, port, tls, config)
            .await
            .map_err(to_wit_failure);
        self.credit_host_time(started.elapsed());
        let opened = opened?;
        let handle = self.next_connection;
        self.next_connection = self.next_connection.saturating_add(1);
        self.connections.insert(handle, opened);
        if let Ok(transfer) = self.transfer_mut() {
            transfer.open_connections += 1;
        }
        Ok(handle)
    }

    async fn read(
        &mut self,
        connection: u32,
        max: u32,
    ) -> Result<Vec<u8>, super::rdownloader::plugin::types::Failure> {
        let limit = max.min(super::MAX_READ_BYTES) as usize;
        let started = Instant::now();
        let socket = self.connections.get_mut(&connection).ok_or_else(|| {
            to_wit_failure(Failure::coded(
                FailureKind::Permanent,
                "plugin.net_unknown_connection",
                "No such connection",
            ))
        })?;
        let bytes = socket.read(limit).await.map_err(to_wit_failure);
        self.credit_host_time(started.elapsed());
        let bytes = bytes?;
        // Pacing before the bytes are handed over shapes what is pulled off the wire, not
        // just what is written afterwards — the same order `rd_ftp::transfer` uses.
        if let Some(transfer) = self.transfer.as_ref()
            && !bytes.is_empty()
        {
            let limiter = transfer.bandwidth.clone();
            let waited = Instant::now();
            if let Err(error) = limiter.acquire(bytes.len()).await {
                tracing::warn!(error = %error, "plugin transfer bandwidth limiter failed");
            }
            self.credit_host_time(waited.elapsed());
        }
        Ok(bytes)
    }

    async fn write(
        &mut self,
        connection: u32,
        bytes: Vec<u8>,
    ) -> Result<u32, super::rdownloader::plugin::types::Failure> {
        let started = Instant::now();
        let socket = self.connections.get_mut(&connection).ok_or_else(|| {
            to_wit_failure(Failure::coded(
                FailureKind::Permanent,
                "plugin.net_unknown_connection",
                "No such connection",
            ))
        })?;
        let written = socket.write(&bytes).await.map_err(to_wit_failure);
        self.credit_host_time(started.elapsed());
        Ok(u32::try_from(written?).unwrap_or(u32::MAX))
    }

    async fn start_tls(
        &mut self,
        connection: u32,
        server_name: String,
    ) -> Result<(), super::rdownloader::plugin::types::Failure> {
        let started = Instant::now();
        let socket = self.connections.get_mut(&connection).ok_or_else(|| {
            to_wit_failure(Failure::coded(
                FailureKind::Permanent,
                "plugin.net_unknown_connection",
                "No such connection",
            ))
        })?;
        let result = socket.upgrade(&server_name).await.map_err(to_wit_failure);
        self.credit_host_time(started.elapsed());
        result
    }

    async fn close(&mut self, connection: u32) {
        if self.connections.remove(&connection).is_some()
            && let Some(transfer) = self.transfer.as_mut()
        {
            transfer.open_connections = transfer.open_connections.saturating_sub(1);
        }
    }
}

/// Reuses the HTTP allowlist's pattern language, so one manifest reads one way throughout.
fn host_allowed(host: &str, patterns: &[String]) -> bool {
    let Ok(url) = url::Url::parse(&format!("https://{host}/")) else {
        return false;
    };
    domain_allowed(&url, patterns)
}

/// Resolves the target and refuses addresses no transfer protocol has a reason to reach.
async fn refuse_local_targets(host: &str, port: u16) -> Result<(), Failure> {
    let target = format!("{host}:{port}");
    let addresses = tokio::net::lookup_host(target).await.map_err(|error| {
        Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "plugin.net_resolve_failed",
            format!("Could not resolve the server name: {error}"),
        )
    })?;
    for address in addresses {
        if connection::is_local_only(address.ip()) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.net_local_target",
                "The connection target resolves to a local address",
            ));
        }
    }
    Ok(())
}

fn refused(capability: &str) -> Failure {
    Failure::coded(
        FailureKind::Unsupported,
        "plugin.capability_not_granted",
        format!("This plugin does not declare the {capability} capability"),
    )
}
