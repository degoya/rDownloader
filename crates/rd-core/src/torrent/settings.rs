//! BitTorrent downloads via the embedded librqbit engine: which links route to it and
//! the engine settings (part of the `service.settings` blob, keys prefixed `torrent_`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Which peer transports the torrent listener accepts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TorrentListenMode {
    TcpOnly,
    UtpOnly,
    #[default]
    TcpAndUtp,
}

/// Provider name stored on torrent candidates.
pub const TORRENT_PROVIDER: &str = "torrent";

/// Content types a `.torrent` document is served with.
///
/// A Torznab result links to its torrent through a query rather than a `.torrent` path, so
/// the declared type is what identifies it (RD-080-11).
pub const TORRENT_CONTENT_TYPES: &[&str] = &["application/x-bittorrent", "application/x-torrent"];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TorrentSettings {
    /// Incoming peer listen port; `None` = a random port. Applied on the next start.
    pub torrent_listen_port: Option<u16>,
    /// Seed until uploaded/downloaded reaches this ratio (0 disables the ratio stop).
    pub torrent_seed_ratio: f64,
    /// Stop seeding after this many minutes; `None` = no time limit.
    pub torrent_seed_time_minutes: Option<u32>,
    /// Seed finished torrents at all; off completes them immediately.
    pub torrent_seeding_enabled: bool,
    /// Upload data to peers at all. **Off by default.**
    ///
    /// Distinct from seeding, which only covers what happens after a torrent finishes: the
    /// engine uploads to peers *while* downloading too, and switching that off is the only
    /// way to run without sharing anything. When this is off, no category or per-torrent
    /// override can turn seeding back on.
    pub torrent_sharing_enabled: bool,
    /// Global upload limit in bytes per second; `None` = unlimited. Applied on next start.
    pub torrent_upload_limit_bytes_per_second: Option<crate::ByteCount>,
    /// Name of the network interface every torrent socket is bound to; `None` binds to
    /// all. Only honoured where the platform supports it, see the capability matrix.
    pub torrent_bind_interface: Option<String>,
    /// Pause all torrent traffic when the bound interface disappears, and resume when it
    /// comes back. The point of binding to a VPN interface.
    pub torrent_kill_switch_enabled: bool,
    /// HTTP(S) URL of an IP blocklist the engine loads at startup.
    pub torrent_ip_blocklist_url: Option<String>,
    /// Which peer transports the listener accepts.
    pub torrent_listen_mode: TorrentListenMode,
    /// Maximum number of peer connections per torrent; `None` leaves it to the engine.
    pub torrent_peer_limit: Option<u32>,
    /// Global download limit in bytes per second; `None` = unlimited.
    pub torrent_download_limit_bytes_per_second: Option<crate::ByteCount>,
    /// SOCKS5 proxy profile for outgoing peer connections. Tracker and metadata traffic
    /// is not covered; see the `tracker_proxy` capability.
    pub torrent_proxy_profile_id: Option<crate::ProxyProfileId>,
    /// Ask the router to forward the listen port via UPnP IGD.
    pub torrent_upnp_enabled: bool,
    /// Port announced to trackers when it differs from the listen port (behind a mapping).
    pub torrent_announce_port: Option<u16>,
    /// Show full peer addresses in the peer list. Off by default: a peer address is
    /// personal data of a third party, and the network prefix is enough to judge a swarm.
    pub torrent_peer_addresses_visible: bool,
    /// Keep stored `.torrent` files after the download finishes (shared blob key,
    /// hence no `torrent_` prefix).
    pub keep_import_history: bool,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self {
            torrent_listen_port: None,
            torrent_seed_ratio: 1.0,
            torrent_seed_time_minutes: None,
            torrent_seeding_enabled: false,
            torrent_sharing_enabled: false,
            torrent_upload_limit_bytes_per_second: None,
            torrent_bind_interface: None,
            torrent_kill_switch_enabled: false,
            torrent_ip_blocklist_url: None,
            torrent_listen_mode: TorrentListenMode::TcpAndUtp,
            torrent_peer_limit: None,
            torrent_download_limit_bytes_per_second: None,
            torrent_proxy_profile_id: None,
            torrent_upnp_enabled: false,
            torrent_announce_port: None,
            torrent_peer_addresses_visible: false,
            keep_import_history: true,
        }
    }
}
