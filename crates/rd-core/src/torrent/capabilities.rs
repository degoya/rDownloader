//! What the embedded BitTorrent engine can actually do.
//!
//! rDownloader models more torrent control than any one engine implements. Rather than
//! silently ignoring an option the engine does not support — which would show the user a
//! switch that does nothing — every such option is gated on this descriptor: the API
//! rejects it with `torrent.capability_unsupported` and the UI disables the control.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Engine feature matrix, reported by `GET /api/v1/torrents/capabilities`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentEngineCapabilities {
    /// Name of the engine backing the session.
    #[schema(example = "librqbit")]
    pub engine: &'static str,
    pub engine_version: &'static str,
    /// Downloading only a subset of the files.
    pub file_selection: bool,
    /// Per-file priority tiers.
    pub file_priorities: bool,
    /// Whether priorities are emulated on top of plain include/exclude rather than being
    /// a native engine feature. Emulated tiers are opened one after another, so files in
    /// one tier still download concurrently.
    pub priorities_emulated: bool,
    pub sequential_download: bool,
    pub first_last_piece: bool,
    /// Editing the tracker list of an existing torrent.
    pub tracker_edit: bool,
    pub tracker_reannounce: bool,
    pub tracker_scrape: bool,
    pub peer_stats: bool,
    pub piece_stats: bool,
    /// Binding all torrent sockets to one network interface.
    pub interface_binding: bool,
    /// Loading an IP blocklist from an HTTP(S) URL.
    pub ip_blocklist_url: bool,
    /// Loading an IP blocklist from a local file.
    pub ip_blocklist_file: bool,
    /// Protocol encryption (MSE/PE).
    pub protocol_encryption: bool,
    /// Routing outgoing peer connections through a SOCKS5 proxy.
    pub socks5_peer_proxy: bool,
    /// Separate proxies for tracker, metadata and peer traffic.
    pub per_class_proxy: bool,
    /// Routing tracker announces through the configured proxy.
    pub tracker_proxy: bool,
    pub upnp: bool,
    pub natpmp: bool,
    pub pcp: bool,
    /// BEP 19 web seeds as a download source. When `false`, web seeds are still parsed and
    /// shown as diagnostics but never fetched.
    pub web_seeds: bool,
}

impl TorrentEngineCapabilities {
    /// Whether the named capability is available. The names match the field names and are
    /// what the API returns in `params.capability`.
    #[must_use]
    pub fn supports(&self, capability: &str) -> bool {
        match capability {
            "file_selection" => self.file_selection,
            "file_priorities" => self.file_priorities,
            "sequential_download" => self.sequential_download,
            "first_last_piece" => self.first_last_piece,
            "tracker_edit" => self.tracker_edit,
            "tracker_reannounce" => self.tracker_reannounce,
            "tracker_scrape" => self.tracker_scrape,
            "peer_stats" => self.peer_stats,
            "piece_stats" => self.piece_stats,
            "interface_binding" => self.interface_binding,
            "ip_blocklist_url" => self.ip_blocklist_url,
            "ip_blocklist_file" => self.ip_blocklist_file,
            "protocol_encryption" => self.protocol_encryption,
            "socks5_peer_proxy" => self.socks5_peer_proxy,
            "per_class_proxy" => self.per_class_proxy,
            "tracker_proxy" => self.tracker_proxy,
            "upnp" => self.upnp,
            "natpmp" => self.natpmp,
            "pcp" => self.pcp,
            "web_seeds" => self.web_seeds,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TorrentEngineCapabilities;

    fn capabilities() -> TorrentEngineCapabilities {
        TorrentEngineCapabilities {
            engine: "test",
            engine_version: "0",
            file_selection: true,
            file_priorities: true,
            priorities_emulated: true,
            sequential_download: false,
            first_last_piece: false,
            tracker_edit: true,
            tracker_reannounce: true,
            tracker_scrape: true,
            peer_stats: true,
            piece_stats: true,
            interface_binding: true,
            ip_blocklist_url: true,
            ip_blocklist_file: false,
            protocol_encryption: false,
            socks5_peer_proxy: true,
            per_class_proxy: false,
            tracker_proxy: false,
            upnp: true,
            natpmp: false,
            pcp: false,
            web_seeds: false,
        }
    }

    #[test]
    fn lookup_matches_the_field_values() {
        let capabilities = capabilities();
        assert!(capabilities.supports("file_selection"));
        assert!(!capabilities.supports("sequential_download"));
        assert!(!capabilities.supports("web_seeds"));
    }

    #[test]
    fn an_unknown_capability_is_never_supported() {
        assert!(!capabilities().supports("teleportation"));
    }
}
