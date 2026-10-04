//! The settings DTO: its fields, the serde defaults of each and the defaults of a fresh
//! installation.

use super::*;

/// Mutable local service settings exposed in version one.
#[derive(Clone, Deserialize, Serialize, ToSchema)]
#[serde(default)]
pub struct SettingsResponse {
    pub max_active_files: u32,
    pub max_chunks_per_file: u32,
    /// Simultaneous connections one host may see across all running transfers; `0` lifts
    /// the limit entirely.
    #[serde(default = "default_connections_per_host")]
    pub max_connections_per_host: u32,
    /// NNTP connections one NZB file may hold at once; `0` is as many as the enabled
    /// servers allow. A value below the servers' total caps the file (RD-108-25).
    pub nntp_connections_per_file: u32,
    /// NZB files downloaded at once, 1 to 8; `0` is automatic - as many as keep every
    /// connection busy (RD-130-22). All of them together take one `max_active_files` slot.
    pub nntp_parallel_files: u32,
    pub speed_limit_bytes_per_second: Option<rd_core::ByteCount>,
    /// Hand-set upload limit for every upload — torrent seeding, object storage, rclone and
    /// upload destinations; empty = unlimited. The stricter of it and the active bandwidth
    /// profile's upload limit wins (RD-150-15).
    pub upload_limit_bytes_per_second: Option<rd_core::ByteCount>,
    pub generate_sha256: bool,
    pub global_proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub custom_ca_pem: Option<String>,
    pub archive_max_files: u32,
    pub archive_max_uncompressed_bytes: rd_core::ByteCount,
    pub rar_executable: Option<String>,
    pub rar_tool: String,
    /// Retries per file before a retryable failure becomes final (0–100). Waiting out a limit
    /// the hoster imposed (a rate or daily limit, an IP block) is no retry and spends none of
    /// them; at most 48 such waits in a row (RD-191-12).
    pub max_retries: u32,
    /// Put failed downloads back into the queue on a timer when their failure may pass by
    /// itself: a limit, an IP block, a server or network that was down, a file reported
    /// offline (RD-191-12).
    #[serde(default)]
    pub auto_retry_failed: bool,
    /// Hours between a download failing and its automatic retry (1–24).
    #[serde(default = "default_auto_retry_interval_hours")]
    pub auto_retry_interval_hours: u32,
    /// Automatic retry rounds per download (0–100); `0` is no limit.
    #[serde(default = "default_auto_retry_max_rounds")]
    pub auto_retry_max_rounds: u32,
    /// Keep NZB import entries and stored .torrent files after the download finishes;
    /// off removes them automatically on completion.
    #[serde(default = "default_keep_import_history")]
    pub keep_import_history: bool,
    /// Remove finished packages from the queue once they have been finished long enough.
    #[serde(default)]
    pub auto_remove_finished: bool,
    /// How long a package stays after it finished, in hours (1–720).
    #[serde(default = "default_auto_remove_delay_hours")]
    pub auto_remove_delay_hours: u32,
    /// Keep a finished package that still holds a file which did not complete.
    #[serde(default = "default_auto_remove_keep_failed")]
    pub auto_remove_keep_failed: bool,
    /// Absolute path of the password list (one per line); empty = `passwords.txt` next to the database.
    pub passwords_file: Option<String>,
    /// Switches the administrator login off (only sensible on a trusted loopback/LAN setup).
    #[serde(default)]
    pub admin_login_disabled: bool,
    /// Address ranges whose `X-Forwarded-For` is believed, as CIDR or bare addresses.
    ///
    /// Empty means no header is read and the peer address is the client, which is the safe
    /// default: without it, anyone could name any client they liked.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// What the outside world calls this service: `https://rd.example.com/downloads`.
    ///
    /// Carries the scheme, host and mount point together so they cannot disagree.
    #[serde(default)]
    pub external_url: Option<String>,
    /// Host names, beyond the external URL's, a browser may call the service by.
    ///
    /// Addresses and `localhost` always pass; any other name in a request's `Host` is refused
    /// unless it is listed here, which is what keeps a DNS rebinding page out.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// When the session cookie is marked `Secure`.
    #[serde(default)]
    pub cookie_security: rd_authn::CookieSecurity,
    /// Hours without a request after which a sign-in ends (RD-130-09); 1 to 720.
    #[serde(default = "default_session_idle_hours")]
    pub session_idle_hours: u32,
    /// Hours from sign-in after which a session ends however busy it is; 1 to 2160.
    ///
    /// A shorter value ends the sessions already past it at once; a longer one applies from
    /// the next sign-in, because the browser keeps the cookie only as long as it was told.
    #[serde(default = "default_session_max_hours")]
    pub session_max_hours: u32,
    /// Post-processing level for packages without an explicit or category level.
    pub default_level: Option<rd_core::PostprocessLevel>,
    /// Hold new downloads while a package is post-processing.
    pub pause_during_postprocess: bool,
    /// Transfer services switched off entirely. A disabled service refuses new links at
    /// intake and blocks whatever it already had queued, with a reason.
    ///
    /// Every one defaults to on except nothing: switching a service off is an explicit act.
    /// Torrent is the one that also shares data, which is why it has its own switch for that
    /// rather than being covered by this one.
    #[serde(default = "default_true")]
    pub torrent_service_enabled: bool,
    #[serde(default = "default_true")]
    pub usenet_service_enabled: bool,
    #[serde(default = "default_true")]
    pub media_service_enabled: bool,
    #[serde(default = "default_true")]
    pub gallery_service_enabled: bool,
    #[serde(default = "default_true")]
    pub recording_service_enabled: bool,
    #[serde(default = "default_true")]
    pub remote_service_enabled: bool,
    /// Extensions (without dot) deleted from the package folder after unpacking.
    pub cleanup_extensions: Vec<String>,
    /// Delete sample files after unpacking and skip sample archives.
    pub ignore_samples: bool,
    /// Also extract archives found inside extracted archives (depth-capped).
    #[serde(default)]
    pub recursive_unpack: bool,
    /// Unpack every archive set into a folder of its own below the package folder, named after
    /// the archive, instead of straight into the package folder. Off by default (RD-170-16).
    #[serde(default)]
    pub unpack_to_subfolder: bool,
    /// Verify the CRC32 checksums of any `.sfv` index in the package before unpacking.
    #[serde(default)]
    pub sfv_verify: bool,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it.
    /// On by default, like SABnzbd's `safe_postproc`; off means the unpack runs anyway and a
    /// broken recovery set beside intact archives no longer locks a package (RD-104-04).
    #[serde(default = "default_true")]
    pub safe_postproc: bool,
    /// Delete the PAR2 recovery set once repair and extraction have both succeeded. Off by
    /// default: it is the only thing that can rescue a damaged package.
    #[serde(default)]
    pub delete_par2: bool,
    /// Download every PAR2 recovery volume of an NZB straight away. Off by default, like
    /// SABnzbd's `enable_all_par`: the main index comes down with the payload, the `vol`
    /// volumes wait, and only a repair that is short of blocks fetches as many of them as the
    /// gap needs (RD-107-04). On restores the older behaviour of fetching all of them.
    #[serde(default)]
    pub enable_all_par: bool,
    /// Post-processing plugin steps enabled by default, by plugin id and in the order they
    /// run. A category may override the list, including with an empty one.
    #[serde(default)]
    pub plugin_steps: Vec<String>,
    /// Whether installed metadata enricher plugins are asked about resolved links
    /// (RD-090-14). Off by default: an enricher reaches a service outside this machine, and
    /// doing that on the strength of having installed a plugin would be a decision nobody
    /// made.
    #[serde(default)]
    pub metadata_enrichment_enabled: bool,
    /// Files containing "sample" in their name count as samples only below this size.
    pub sample_max_bytes: rd_core::ByteCount,
    /// Absolute scripts directory; empty = `scripts` next to the database.
    pub scripts_directory: Option<String>,
    /// Seconds after which a post-processing script is killed (10–86400).
    pub script_timeout_seconds: u32,
    /// Absolute path of yt-dlp; empty = look up on PATH.
    pub media_ytdlp_executable: Option<String>,
    /// Absolute path of ffmpeg; empty = look up on PATH.
    pub media_ffmpeg_executable: Option<String>,
    /// Variant preselected for new media links (`best`, `1080p`, `720p`, `audio_mp3`, …).
    pub media_default_variant: String,
    /// Full default selection for new media links; `None` uses `media_default_variant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_default_criteria: Option<rd_core::MediaFormatCriteria>,
    /// Default output template for media downloads; `None` keeps the plain file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_output_template: Option<String>,
    /// Hosts handled by the media provider (without `www.`).
    pub media_hosts: Vec<String>,
    /// Concurrent media downloads (1–8).
    pub media_max_parallel: u32,
    /// Timeout of one metadata probe in seconds (5–600).
    pub media_check_timeout_seconds: u32,
    /// Absolute path of gallery-dl; empty = look up in the vendor folders and on PATH.
    #[serde(default)]
    pub gallery_executable: Option<String>,
    /// Hosts handled by the gallery provider (without `www.`).
    #[serde(default = "rd_core::GallerySettings::default_hosts")]
    pub gallery_hosts: Vec<String>,
    /// Concurrent gallery downloads (1–8).
    #[serde(default = "default_gallery_max_parallel")]
    pub gallery_max_parallel: u32,
    /// Concurrent FTP/SFTP transfers (1–8).
    #[serde(default = "default_remote_max_parallel")]
    pub remote_max_parallel: u32,
    /// Connect, login and per-read timeout for FTP/SFTP in seconds (5–600).
    #[serde(default = "default_remote_timeout")]
    pub remote_timeout_seconds: u32,
    /// Whether an unknown SSH host key may be trusted on first use without asking.
    #[serde(default)]
    pub remote_ssh_auto_trust: bool,
    /// Absolute path of streamlink; empty = vendor folders (incl. `vendor/streamlink/bin`)
    /// and PATH.
    #[serde(default)]
    pub record_streamlink_executable: Option<String>,
    /// Stream selection used when a channel has none (`best`, `1080p`, …).
    #[serde(default = "default_record_quality")]
    pub record_default_quality: String,
    /// Seconds between liveness probes of enabled channels (60–3600).
    #[serde(default = "default_record_poll_interval")]
    pub record_poll_interval_seconds: u32,
    /// Concurrent recordings (1–8); recordings never block regular downloads.
    #[serde(default = "default_record_max_parallel")]
    pub record_max_parallel: u32,
    /// Incoming BitTorrent peer port; empty = a random port. Applied on the next start.
    #[serde(default)]
    pub torrent_listen_port: Option<u16>,
    /// Seed until uploaded/downloaded reaches this ratio (0 disables the ratio stop).
    #[serde(default = "default_torrent_seed_ratio")]
    pub torrent_seed_ratio: f64,
    /// Stop seeding after this many minutes; empty = no time limit.
    #[serde(default)]
    pub torrent_seed_time_minutes: Option<u32>,
    /// Seed finished torrents; off completes them immediately after the download.
    #[serde(default = "default_torrent_seeding_enabled")]
    pub torrent_seeding_enabled: bool,
    /// Upload data to peers at all. **Off by default**, so a fresh installation shares
    /// nothing. Distinct from seeding, which only covers the phase after a download
    /// finishes: the engine uploads while downloading too, and this is what stops it.
    #[serde(default)]
    pub torrent_sharing_enabled: bool,
    /// Global torrent upload limit in bytes per second; empty = unlimited. Next start.
    #[serde(default)]
    pub torrent_upload_limit_bytes_per_second: Option<rd_core::ByteCount>,
    /// Network interface every torrent socket binds to; `None` binds to all.
    #[serde(default)]
    pub torrent_bind_interface: Option<String>,
    /// Pause torrent traffic when the bound interface disappears.
    #[serde(default)]
    pub torrent_kill_switch_enabled: bool,
    /// HTTP(S) URL of an IP blocklist the torrent engine loads at startup.
    #[serde(default)]
    pub torrent_ip_blocklist_url: Option<String>,
    #[serde(default)]
    pub torrent_listen_mode: rd_core::TorrentListenMode,
    #[serde(default)]
    pub torrent_peer_limit: Option<u32>,
    #[serde(default)]
    pub torrent_download_limit_bytes_per_second: Option<rd_core::ByteCount>,
    /// SOCKS5 proxy profile for outgoing torrent peer connections.
    #[serde(default)]
    pub torrent_proxy_profile_id: Option<rd_core::ProxyProfileId>,
    /// Ask the router to forward the listen port via UPnP.
    #[serde(default)]
    pub torrent_upnp_enabled: bool,
    /// Port announced to trackers when a mapping uses a different external port.
    #[serde(default)]
    pub torrent_announce_port: Option<u16>,
    /// Show full peer addresses in the torrent peer list instead of the network prefix.
    #[serde(default)]
    pub torrent_peer_addresses_visible: bool,
    /// Weekly windows during which resource-intensive work waits.
    #[serde(default)]
    pub quiet_hours: rd_limits::QuietHours,
    /// Hold back PAR2 repair, unpacking and uploads during quiet hours.
    #[serde(default = "default_true")]
    pub quiet_hours_defer_postprocess: bool,
    /// Group notification deliveries until the quiet period ends.
    #[serde(default = "default_true")]
    pub quiet_hours_defer_notifications: bool,
    /// What runs once the queue and post-processing have drained.
    #[serde(default)]
    pub completion_action: rd_power::CompletionAction,
    /// Script name inside the post-processing scripts directory.
    #[serde(default)]
    pub completion_script: Option<String>,
    /// Seconds a power action counts down before it runs (10–3600).
    #[serde(default = "default_completion_countdown")]
    pub completion_countdown_seconds: u32,
    /// Local approval for standby and shutdown; without it they are never executed.
    #[serde(default)]
    pub power_actions_allowed: bool,
    /// Hold the queue while the machine runs on battery.
    #[serde(default)]
    pub pause_on_battery: bool,
    /// Hold the queue while the connection reports itself as metered.
    #[serde(default)]
    pub pause_on_metered: bool,
    /// Keep the machine awake while downloads or post-processing are actually running.
    #[serde(default)]
    pub prevent_standby: bool,
    /// Keep the display awake too, for a machine somebody watches.
    #[serde(default)]
    pub prevent_display_standby: bool,
    /// Treat links in a package that point at the same file as alternatives, downloading one.
    #[serde(default = "default_mirror_detection")]
    pub mirror_detection: bool,
    /// Run the reconnect script when free downloads are stuck behind an IP limit.
    #[serde(default)]
    pub reconnect_enabled: bool,
    /// Script name inside the post-processing scripts directory.
    #[serde(default)]
    pub reconnect_script: Option<String>,
    /// Weekly windows a reconnect may run in; empty means any time.
    ///
    /// The element type is shared with quiet hours, which is the same shape — a weekday set
    /// and a span of local minutes — and lets the interface reuse the same editor.
    #[serde(default)]
    pub reconnect_windows: Vec<rd_limits::QuietWindow>,
    /// Shortest gap between two reconnects, in minutes (1–1440).
    #[serde(default = "default_reconnect_interval_minutes")]
    pub reconnect_min_interval_minutes: u32,
    /// How long a reconnect may take before it is given up on, in seconds (30–900).
    #[serde(default = "default_reconnect_timeout_seconds")]
    pub reconnect_timeout_seconds: u32,
    /// Allow a reconnect while transfers are running; they are paused and resumed around it.
    #[serde(default)]
    pub reconnect_abort_active: bool,
    /// Addresses asked what the public address is; empty uses the built-in list.
    #[serde(default)]
    pub reconnect_ip_check_urls: Vec<String>,
    /// IANA timezone the bandwidth schedule and its budget periods are read in.
    #[serde(default = "default_bandwidth_timezone")]
    pub bandwidth_timezone: String,
    /// Bandwidth profile applied outside every schedule window; empty = no limits.
    #[serde(default)]
    pub bandwidth_default_profile_id: Option<rd_core::BandwidthProfileId>,
    /// Free space that must remain on a storage root without an own threshold.
    #[serde(default = "default_storage_minimum_free_bytes")]
    pub storage_minimum_free_bytes: rd_core::ByteCount,
    /// Release a blocked storage root by itself once space is free again.
    #[serde(default = "default_storage_auto_resume")]
    pub storage_auto_resume: bool,
    /// A transfer of unknown size may start while `threshold × factor` bytes are free (1–64).
    #[serde(default = "default_storage_unknown_size_headroom")]
    pub storage_unknown_size_headroom: u32,
    /// What happens when a finished file meets a name that is taken, for packages and
    /// categories without a policy of their own (RD-150-01).
    #[serde(default)]
    pub storage_collision_policy: rd_core::CollisionPolicy,
    /// Absolute directory searched for yt-dlp/ffmpeg/ffprobe/unrar/7z before `PATH`; empty =
    /// the built-in `vendor` folders next to the executable and in the data directory.
    pub vendor_directory: Option<String>,
    /// Whether this installation may download, verify and activate tool versions itself
    /// (RD-102-02). Off by default: fetching executables is not something to start unasked.
    #[serde(default)]
    pub managed_tools_enabled: bool,
    /// `https://` URL of the signed tool manifest; empty = only the manifest compiled into
    /// this build, which is the offline-safe default.
    #[serde(default)]
    pub managed_tools_manifest_url: Option<String>,
    /// Tools whose compatibility verdict is reported but not enforced (RD-102-03). The
    /// warning stays; only the block is dropped, and every skipped block is logged.
    #[serde(default)]
    pub tool_compatibility_overrides: Vec<String>,
    /// Upload finished packages to an rclone remote as the last post-processing step.
    #[serde(default)]
    pub upload_enabled: bool,
    /// rclone target in `remote:path` form; the package folder is created below it.
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// `copy` keeps the local files, `move` removes them after a successful upload.
    #[serde(default = "default_upload_mode")]
    pub upload_mode: String,
    /// Absolute path of rclone; empty = look up in the vendor folders and on PATH.
    #[serde(default)]
    pub rclone_executable: Option<String>,
    /// Scan every finished package with ClamAV before it counts as finished (RD-190-14). A
    /// finding fails the package and stops everything after the scan; a `clamd` that cannot be
    /// reached is a warning on the step and the package carries on. Off by default.
    pub malware_scan_enabled: bool,
    /// Where `clamd` listens: `host:port`, or `unix:/path` for its local socket; empty =
    /// `127.0.0.1:3310`. Nothing but the files' bytes goes there, and nothing goes anywhere else.
    pub clamd_address: Option<String>,
    /// Largest file streamed to `clamd`; a larger one is not scanned and the step counts it.
    /// `clamd`'s `StreamMaxLength` has to be at least this.
    pub malware_scan_max_bytes: rd_core::ByteCount,
    /// Seconds one exchange with `clamd` may take: the connection, a chunk, the verdict (5–3600).
    pub malware_scan_timeout_seconds: u32,
    /// Absolute path of the domain blocklist (one host per line); empty =
    /// `excluded_domains.txt` next to the database.
    pub excluded_domains_file: Option<String>,
    /// Import `.dlc` containers. Off by default on purpose: the format cannot be decrypted
    /// locally, so every import sends the container's key to the service below.
    #[serde(default)]
    pub dlc_service_enabled: bool,
    /// `dlcrypt` service that unwraps the container key; empty = the JDownloader service.
    #[serde(default)]
    pub dlc_service_endpoint: Option<String>,
    /// Show the cover images an indexer announces for its hits (RD-101-17).
    ///
    /// On by default, unlike the enricher switch next to it, because nothing is fetched on
    /// the strength of it: the addresses arrive with the search answer the subscription
    /// already makes, and only the browser loads the pictures, lazily. It is still a switch
    /// because loading them tells the indexer which hits are on somebody's screen, and the
    /// details themselves stay visible when it is off.
    #[serde(default = "default_true")]
    pub subscription_item_images_enabled: bool,
    /// Whether the LinkGrabber offers to hand an NZB import to a remote-job provider: the menu
    /// in each NZB row and the entry in the selection bar (RD-191-13).
    ///
    /// On by default, because it is offered only where an account takes NZB files. It is a
    /// display choice and nothing more: the badge of an import already handed over stays, and
    /// the REST route and the MCP tool keep working, so switching it off cannot strand a job.
    #[serde(default = "default_true")]
    pub nzb_hand_over_linkgrabber_enabled: bool,
    /// The same offer in the Downloads view, in the menu of a package that came from an NZB
    /// (RD-191-13). A switch of its own (owner, 2026-10-04): handing over a failed package is a
    /// different habit from handing over before the queue.
    #[serde(default = "default_true")]
    pub nzb_hand_over_downloads_enabled: bool,
    /// Port the web UI listens on; applied on the next start. `--listen`/`RDOWNLOADER_LISTEN`
    /// override it.
    pub ui_port: Option<u16>,
    /// How byte counts are rendered in the interface: `binary` (KiB/MiB/GiB, 1024) or
    /// `decimal` (kB/MB/GB, 1000). Purely a display choice — nothing computes with it.
    #[serde(default = "default_byte_display")]
    pub byte_display: String,
    /// Which magnitude of the ladder byte counts are printed in: `auto` picks the step that
    /// fits each value, while `byte`, `kilo`, `mega`, `giga`, `tera` or `peta` pin every value
    /// to that step so a list of sizes can be compared column by column (RD-106-14).
    ///
    /// Named by magnitude rather than by unit because the unit names belong to `byte_display`:
    /// `mega` reads as MiB on the binary ladder and MB on the decimal one.
    #[serde(default = "default_byte_unit")]
    pub byte_unit: String,
    /// Whether the browser tab reports what is running — the queue rate and the number of
    /// active transfers — instead of the application name alone (RD-106-07).
    ///
    /// On by default: a tab that says nothing is what every earlier version had, and the point
    /// of the feature is not having to bring the window forward. It is a switch because a title
    /// that keeps changing is a distraction for some people, and that is not arguable.
    #[serde(default = "default_true")]
    pub title_status_enabled: bool,
    /// Plugin ids the user switched off. They stay installed and listed — otherwise they could
    /// not be switched back on — but are not loaded, compiled or executed.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    /// Days the transfer statistics keep hourly buckets before folding them into daily ones
    /// (RD-110-01); 1 to the retention.
    #[serde(default = "default_stats_hourly_days")]
    pub stats_hourly_days: u32,
    /// Days the transfer statistics are kept at all (RD-110-01); 7 to 3650. The all-time
    /// totals behind the metrics counters are never thinned.
    #[serde(default = "default_stats_retention_days")]
    pub stats_retention_days: u32,
    /// Log records the structured log store keeps at most (1000-500000); the oldest go
    /// first (RD-110-02).
    #[serde(default = "default_log_retention_records")]
    pub log_retention_records: u32,
    /// Days a log record is kept at most (1-365), whatever the count.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// Audit records the append-only audit log keeps at most (10000-2000000); the oldest
    /// whole records go first (RD-110-03).
    #[serde(default = "default_audit_retention_records")]
    pub audit_retention_records: u32,
    /// Days an audit record is kept at most (30-3650), whatever the count.
    #[serde(default = "default_audit_retention_days")]
    pub audit_retention_days: u32,
    /// Whether finished spans are exported over OTLP (RD-110-03). Off by default and off
    /// after an upgrade: exporting traces sends the shape of a person's activity to a third
    /// system, which is a decision somebody takes rather than one they discover.
    #[serde(default)]
    pub otlp_enabled: bool,
    /// The collector's OTLP/HTTP traces endpoint, such as
    /// `http://127.0.0.1:4318/v1/traces`. Empty means unconfigured, which is the same as off.
    #[serde(default)]
    pub otlp_endpoint: String,
    /// How long one export attempt may take before it is abandoned (1-60).
    #[serde(default = "default_otlp_timeout_seconds")]
    pub otlp_timeout_seconds: u32,
    /// Seconds between two reconciliation scans of every watched folder (RD-110-31); 5 to
    /// 3600, one value for all folders, taken over by running watchers without a restart.
    #[serde(default = "default_hotfolder_poll_seconds")]
    pub hotfolder_poll_seconds: u32,
    /// Whether the service checks for a new version by itself (RD-180-01). On by default: a
    /// check fetches a public, signed file and sends nothing about the installation. "Check
    /// now" works either way.
    #[serde(default = "default_true")]
    pub update_check_enabled: bool,
    /// Which releases the update check offers: `stable`, or `beta` for the pre-releases too.
    /// Unset, it is `beta` on a pre-release build and `stable` on every other.
    #[serde(default = "default_update_channel")]
    pub update_channel: String,
    /// Hours between two automatic update checks (1-168).
    #[serde(default = "default_update_check_interval_hours")]
    pub update_check_interval_hours: u32,
}

/// Bounds of the two retention settings, in days.
pub const MIN_HOURLY_DAYS: u32 = 1;
pub const MIN_RETENTION_DAYS: u32 = 7;
pub const MAX_RETENTION_DAYS: u32 = 3650;
pub(crate) const DEFAULT_HOURLY_DAYS: u32 = 30;
pub(crate) const DEFAULT_RETENTION_DAYS: u32 = 365;

fn default_stats_hourly_days() -> u32 {
    DEFAULT_HOURLY_DAYS
}

fn default_stats_retention_days() -> u32 {
    DEFAULT_RETENTION_DAYS
}

const fn default_hotfolder_poll_seconds() -> u32 {
    rd_core::DEFAULT_HOTFOLDER_POLL_SECONDS
}

fn default_update_channel() -> String {
    rd_update::UpdateSettings::default().update_channel
}

const fn default_update_check_interval_hours() -> u32 {
    rd_update::settings::DEFAULT_INTERVAL_HOURS
}

impl Default for SettingsResponse {
    fn default() -> Self {
        Self {
            max_active_files: 3,
            max_chunks_per_file: 4,
            log_retention_records: default_log_retention_records(),
            log_retention_days: default_log_retention_days(),
            audit_retention_records: default_audit_retention_records(),
            audit_retention_days: default_audit_retention_days(),
            otlp_enabled: false,
            otlp_endpoint: String::new(),
            otlp_timeout_seconds: default_otlp_timeout_seconds(),
            hotfolder_poll_seconds: default_hotfolder_poll_seconds(),
            update_check_enabled: true,
            update_channel: default_update_channel(),
            update_check_interval_hours: default_update_check_interval_hours(),
            max_connections_per_host: default_connections_per_host(),
            nntp_connections_per_file: 0,
            nntp_parallel_files: 0,
            speed_limit_bytes_per_second: None,
            upload_limit_bytes_per_second: None,
            generate_sha256: true,
            global_proxy_profile_id: None,
            custom_ca_pem: None,
            archive_max_files: 20_000,
            archive_max_uncompressed_bytes: rd_core::ByteCount::new(100 * 1024 * 1024 * 1024)
                .expect("default archive limit fits SQLite"),
            rar_executable: None,
            rar_tool: "unrar".to_owned(),
            max_retries: rd_scheduler::DEFAULT_MAX_RETRIES,
            auto_retry_failed: false,
            auto_retry_interval_hours: default_auto_retry_interval_hours(),
            auto_retry_max_rounds: default_auto_retry_max_rounds(),
            keep_import_history: default_keep_import_history(),
            auto_remove_finished: false,
            auto_remove_delay_hours: default_auto_remove_delay_hours(),
            auto_remove_keep_failed: default_auto_remove_keep_failed(),
            passwords_file: None,
            admin_login_disabled: false,
            trusted_proxies: Vec::new(),
            external_url: None,
            allowed_hosts: Vec::new(),
            cookie_security: rd_authn::CookieSecurity::default(),
            session_idle_hours: default_session_idle_hours(),
            session_max_hours: default_session_max_hours(),
            default_level: Some(rd_core::PostprocessLevel::Unpack),
            pause_during_postprocess: true,
            torrent_service_enabled: true,
            usenet_service_enabled: true,
            media_service_enabled: true,
            gallery_service_enabled: true,
            recording_service_enabled: true,
            remote_service_enabled: true,
            cleanup_extensions: rd_core::PostprocessSettings::default_cleanup_extensions(),
            ignore_samples: true,
            recursive_unpack: false,
            unpack_to_subfolder: false,
            sfv_verify: true,
            safe_postproc: true,
            delete_par2: false,
            enable_all_par: false,
            plugin_steps: Vec::new(),
            metadata_enrichment_enabled: false,
            sample_max_bytes: rd_core::ByteCount::new(300 * 1024 * 1024)
                .expect("sample limit fits SQLite"),
            scripts_directory: None,
            script_timeout_seconds: 3600,
            media_ytdlp_executable: None,
            media_ffmpeg_executable: None,
            media_default_variant: "best".to_owned(),
            media_default_criteria: None,
            media_output_template: None,
            media_hosts: rd_core::MediaSettings::default_hosts(),
            media_max_parallel: 2,
            media_check_timeout_seconds: 60,
            gallery_executable: None,
            gallery_hosts: rd_core::GallerySettings::default_hosts(),
            gallery_max_parallel: default_gallery_max_parallel(),
            remote_max_parallel: default_remote_max_parallel(),
            remote_timeout_seconds: default_remote_timeout(),
            remote_ssh_auto_trust: false,
            record_streamlink_executable: None,
            record_default_quality: default_record_quality(),
            record_poll_interval_seconds: default_record_poll_interval(),
            record_max_parallel: default_record_max_parallel(),
            torrent_listen_port: None,
            torrent_seed_ratio: default_torrent_seed_ratio(),
            torrent_seed_time_minutes: None,
            torrent_seeding_enabled: default_torrent_seeding_enabled(),
            torrent_sharing_enabled: false,
            torrent_upload_limit_bytes_per_second: None,
            torrent_bind_interface: None,
            torrent_kill_switch_enabled: false,
            torrent_ip_blocklist_url: None,
            torrent_listen_mode: rd_core::TorrentListenMode::default(),
            torrent_peer_limit: None,
            torrent_download_limit_bytes_per_second: None,
            torrent_proxy_profile_id: None,
            torrent_upnp_enabled: false,
            torrent_announce_port: None,
            torrent_peer_addresses_visible: false,
            quiet_hours: rd_limits::QuietHours::default(),
            quiet_hours_defer_postprocess: true,
            quiet_hours_defer_notifications: true,
            completion_action: rd_power::CompletionAction::None,
            completion_script: None,
            completion_countdown_seconds: default_completion_countdown(),
            power_actions_allowed: false,
            pause_on_battery: false,
            pause_on_metered: false,
            prevent_standby: false,
            prevent_display_standby: false,
            mirror_detection: default_mirror_detection(),
            reconnect_enabled: false,
            reconnect_script: None,
            reconnect_windows: Vec::new(),
            reconnect_min_interval_minutes: default_reconnect_interval_minutes(),
            reconnect_timeout_seconds: default_reconnect_timeout_seconds(),
            reconnect_abort_active: false,
            reconnect_ip_check_urls: Vec::new(),
            bandwidth_timezone: default_bandwidth_timezone(),
            bandwidth_default_profile_id: None,
            storage_minimum_free_bytes: default_storage_minimum_free_bytes(),
            storage_auto_resume: default_storage_auto_resume(),
            storage_unknown_size_headroom: default_storage_unknown_size_headroom(),
            storage_collision_policy: rd_core::CollisionPolicy::default(),
            byte_display: default_byte_display(),
            byte_unit: default_byte_unit(),
            title_status_enabled: true,
            disabled_plugins: Vec::new(),
            stats_hourly_days: default_stats_hourly_days(),
            stats_retention_days: default_stats_retention_days(),
            vendor_directory: None,
            managed_tools_enabled: false,
            managed_tools_manifest_url: None,
            tool_compatibility_overrides: Vec::new(),
            upload_enabled: false,
            upload_remote: None,
            upload_mode: default_upload_mode(),
            rclone_executable: None,
            malware_scan_enabled: false,
            clamd_address: None,
            malware_scan_max_bytes: rd_core::ByteCount::new(
                rd_core::DEFAULT_MALWARE_SCAN_MAX_BYTES,
            )
            .expect("scan limit fits SQLite"),
            malware_scan_timeout_seconds: rd_core::DEFAULT_MALWARE_SCAN_TIMEOUT_SECONDS,
            excluded_domains_file: None,
            dlc_service_enabled: false,
            subscription_item_images_enabled: true,
            nzb_hand_over_linkgrabber_enabled: true,
            nzb_hand_over_downloads_enabled: true,
            dlc_service_endpoint: None,
            ui_port: None,
        }
    }
}

fn default_upload_mode() -> String {
    "copy".to_owned()
}

fn default_gallery_max_parallel() -> u32 {
    2
}

const fn default_remote_max_parallel() -> u32 {
    2
}

const fn default_remote_timeout() -> u32 {
    60
}

fn default_record_quality() -> String {
    "best".to_owned()
}

fn default_record_poll_interval() -> u32 {
    120
}

fn default_record_max_parallel() -> u32 {
    2
}

fn default_torrent_seed_ratio() -> f64 {
    1.0
}

const fn default_completion_countdown() -> u32 {
    rd_power::DEFAULT_COMPLETION_COUNTDOWN
}

fn default_bandwidth_timezone() -> String {
    rd_core::DEFAULT_BANDWIDTH_TIMEZONE.to_owned()
}

fn default_storage_minimum_free_bytes() -> rd_core::ByteCount {
    rd_core::StorageSettings::default().storage_minimum_free_bytes
}

fn default_storage_auto_resume() -> bool {
    rd_core::StorageSettings::default().storage_auto_resume
}

fn default_byte_display() -> String {
    // Binary keeps the figures the previous versions showed; switching the default would make
    // every size in every installation change overnight for no reason the user asked for.
    "binary".to_owned()
}

fn default_byte_unit() -> String {
    // Scaling every value on its own is what the interface has always done; pinning a unit is
    // the deliberate choice of somebody who wants a column to compare.
    "auto".to_owned()
}

fn default_storage_unknown_size_headroom() -> u32 {
    rd_core::DEFAULT_UNKNOWN_SIZE_HEADROOM
}

fn default_torrent_seeding_enabled() -> bool {
    false
}

/// On: fetching the same bytes twice is never what somebody wanted, and a mirror that is not
/// needed costs nothing where it waits.
const fn default_mirror_detection() -> bool {
    true
}

/// Ten minutes: a router needs a minute or two to come back, and reconnecting in a tight loop
/// against a hoster that is simply refusing gains nothing.
const fn default_reconnect_interval_minutes() -> u32 {
    10
}

/// Three minutes covers a router reboot; past that something else is wrong.
const fn default_reconnect_timeout_seconds() -> u32 {
    180
}

const fn default_session_idle_hours() -> u32 {
    rd_core::DEFAULT_SESSION_IDLE_HOURS
}

const fn default_session_max_hours() -> u32 {
    rd_core::DEFAULT_SESSION_MAX_HOURS
}

/// A day: long enough to notice a finished package, short enough to keep the queue readable.
const fn default_auto_remove_delay_hours() -> u32 {
    24
}

const fn default_auto_retry_interval_hours() -> u32 {
    rd_scheduler::DEFAULT_AUTO_RETRY_INTERVAL_HOURS
}

const fn default_auto_retry_max_rounds() -> u32 {
    rd_scheduler::DEFAULT_AUTO_RETRY_MAX_ROUNDS
}

const fn default_log_retention_records() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_RECORDS
}

const fn default_log_retention_days() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_DAYS
}

const fn default_audit_retention_records() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_RECORDS
}

const fn default_audit_retention_days() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_DAYS
}

const fn default_otlp_timeout_seconds() -> u32 {
    rd_core::DEFAULT_OTLP_TIMEOUT_SECONDS
}

/// Errors are worth looking at, so a package holding one is kept until it is dealt with.
const fn default_auto_remove_keep_failed() -> bool {
    true
}

fn default_keep_import_history() -> bool {
    true
}

/// A settings document written before the per-host limit existed gets the default rather
/// than an unbounded zero.
const fn default_connections_per_host() -> u32 {
    rd_http::DEFAULT_CONNECTIONS_PER_HOST as u32
}
