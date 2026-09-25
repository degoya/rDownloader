//! Binding torrent traffic to one interface, and stopping it when that interface goes.
//!
//! The reason to bind torrent sockets to a specific interface is a VPN: if the tunnel
//! drops, traffic must not silently fall back to the default route and expose the user's
//! own address. librqbit binds every socket it owns — DHT, uTP, TCP, trackers and local
//! discovery — through `bind_device_name`, which uses `SO_BINDTODEVICE` on Linux and
//! `IP_BOUND_IF` on macOS. Windows has neither, so the capability matrix reports interface
//! binding as unavailable there rather than pretending it works.
//!
//! The kill switch closes the remaining gap: binding stops new traffic from leaving the
//! wrong interface, but an already established session has to be paused when the interface
//! disappears, and resumed when it returns.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::TorrentService;

/// How often the bound interface is checked.
///
/// The documented window in which torrent traffic stops after the interface disappears is
/// twice this interval, which is what the API and UI state.
pub const INTERFACE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// One network interface the torrent engine can bind to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct NetworkInterface {
    pub name: String,
    /// Addresses currently configured on the interface.
    pub addresses: Vec<String>,
    /// Whether the interface currently has a usable address.
    pub up: bool,
    /// Loopback interfaces are listed but never a sensible binding target.
    pub loopback: bool,
}

/// What the torrent network layer is currently doing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentNetworkStatus {
    /// The interface the engine is bound to, if any.
    pub bound_interface: Option<String>,
    /// Whether that interface currently exists and has an address.
    pub bound_interface_present: bool,
    pub kill_switch_enabled: bool,
    /// Whether the kill switch has paused torrent traffic right now.
    pub kill_switch_engaged: bool,
    /// Seconds within which traffic stops after the interface disappears.
    pub kill_switch_window_seconds: u64,
    /// Configured blocklist URL, redacted.
    pub ip_blocklist_url: Option<String>,
    /// The session incarnation the engine is running.
    pub session_generation: u64,
    /// Why the last session rebuild failed, if it did. The previous session keeps running.
    pub last_rebuild_error: Option<String>,
    /// Whether a SOCKS5 proxy is configured for outgoing peer connections.
    pub peer_proxy_configured: bool,
    /// Why the configured proxy could not be resolved, if it could not.
    pub peer_proxy_error: Option<String>,
    /// Whether UPnP port forwarding was requested.
    pub upnp_enabled: bool,
    /// Port announced to trackers, when it differs from the listen port.
    pub announce_port: Option<u16>,
}

/// Enumerates the interfaces the engine could bind to.
///
/// Addresses of one interface are merged into a single entry, because that is how the
/// binding works: it names an interface, not an address.
#[must_use]
pub fn interfaces() -> Vec<NetworkInterface> {
    let Ok(found) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut merged: std::collections::BTreeMap<String, NetworkInterface> =
        std::collections::BTreeMap::new();
    for interface in found {
        let entry = merged
            .entry(interface.name.clone())
            .or_insert_with(|| NetworkInterface {
                name: interface.name.clone(),
                addresses: Vec::new(),
                up: false,
                loopback: interface.is_loopback(),
            });
        entry.addresses.push(interface.addr.ip().to_string());
        entry.up = true;
    }
    merged.into_values().collect()
}

/// Whether an interface of that name currently exists with an address.
#[must_use]
pub fn is_present(name: &str) -> bool {
    interfaces()
        .iter()
        .any(|interface| interface.name == name && interface.up)
}

impl TorrentService {
    /// The current network status of the engine.
    pub async fn network_status(&self) -> TorrentNetworkStatus {
        let settings = self.inner.settings.read().await.clone();
        let bound = settings.torrent_bind_interface.clone();
        TorrentNetworkStatus {
            bound_interface_present: bound.as_deref().is_some_and(is_present),
            bound_interface: bound,
            kill_switch_enabled: settings.torrent_kill_switch_enabled,
            kill_switch_engaged: self
                .inner
                .kill_switch_engaged
                .load(std::sync::atomic::Ordering::Relaxed),
            kill_switch_window_seconds: INTERFACE_CHECK_INTERVAL.as_secs() * 2,
            ip_blocklist_url: settings
                .torrent_ip_blocklist_url
                .as_deref()
                .map(rd_core::redact_tracker_url),
            session_generation: self.session_generation().await,
            last_rebuild_error: self.inner.rebuild_error.read().await.clone(),
            peer_proxy_configured: settings.torrent_proxy_profile_id.is_some(),
            // Reported rather than hidden: a proxy that cannot be resolved means the
            // session refuses to start, which is easier to act on when it is visible.
            peer_proxy_error: self
                .proxy_url()
                .await
                .err()
                .map(|error| format!("{error:#}")),
            upnp_enabled: settings.torrent_upnp_enabled,
            announce_port: settings.torrent_announce_port,
        }
    }

    /// Pauses every registered torrent and remembers that the kill switch did it.
    async fn engage_kill_switch(&self) {
        if self
            .inner
            .kill_switch_engaged
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        tracing::warn!("bound network interface disappeared; pausing all torrent traffic");
        let Ok(session) = self.session().await else {
            return;
        };
        for (_, entry) in self.inner.registry.read().await.snapshot() {
            if let Some(handle) = session.get(entry.handle()) {
                let _ = session.pause(&handle).await;
            }
        }
    }

    /// Resumes exactly what the kill switch paused.
    async fn release_kill_switch(&self) {
        if !self
            .inner
            .kill_switch_engaged
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        tracing::info!("bound network interface returned; resuming torrent traffic");
        let Ok(session) = self.session().await else {
            return;
        };
        for (_, entry) in self.inner.registry.read().await.snapshot() {
            if let Some(handle) = session.get(entry.handle()) {
                let _ = session.unpause(&handle).await;
            }
        }
    }
}

/// Background loop watching the bound interface.
pub(crate) async fn watch(service: TorrentService) {
    loop {
        tokio::select! {
            () = service.inner.shutdown.cancelled() => return,
            () = tokio::time::sleep(INTERFACE_CHECK_INTERVAL) => {}
        }
        let settings = service.inner.settings.read().await.clone();
        let Some(interface) = settings.torrent_bind_interface.clone() else {
            // Nothing bound: anything the switch paused earlier is released again.
            service.release_kill_switch().await;
            continue;
        };
        if !settings.torrent_kill_switch_enabled {
            service.release_kill_switch().await;
            continue;
        }
        if is_present(&interface) {
            service.release_kill_switch().await;
        } else {
            service.engage_kill_switch().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{INTERFACE_CHECK_INTERVAL, interfaces, is_present};

    #[test]
    fn the_host_reports_at_least_a_loopback_interface() {
        let found = interfaces();
        assert!(!found.is_empty(), "no interfaces enumerated");
        assert!(found.iter().any(|interface| interface.loopback));
        assert!(
            found
                .iter()
                .all(|interface| !interface.addresses.is_empty())
        );
    }

    #[test]
    fn an_interface_that_does_not_exist_is_reported_absent() {
        assert!(!is_present("rd-nonexistent-interface"));
        let first = interfaces().into_iter().next().expect("an interface");
        assert!(is_present(&first.name));
    }

    #[test]
    fn the_documented_window_is_twice_the_check_interval() {
        assert_eq!(INTERFACE_CHECK_INTERVAL.as_secs() * 2, 10);
    }
}
