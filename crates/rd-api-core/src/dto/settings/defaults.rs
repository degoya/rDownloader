//! The defaults of a fresh installation and the serde default of each field.

use super::*;

pub(super) fn default_stats_hourly_days() -> u32 {
    DEFAULT_HOURLY_DAYS
}

pub(super) fn default_stats_retention_days() -> u32 {
    DEFAULT_RETENTION_DAYS
}

pub(super) const fn default_hotfolder_poll_seconds() -> u32 {
    rd_core::DEFAULT_HOTFOLDER_POLL_SECONDS
}

pub(super) fn default_update_channel() -> String {
    rd_update::UpdateSettings::default().update_channel
}

pub(super) const fn default_update_check_interval_hours() -> u32 {
    rd_update::settings::DEFAULT_INTERVAL_HOURS
}

pub(super) const fn default_history_retention_entries() -> u32 {
    rd_core::DEFAULT_HISTORY_RETENTION_ENTRIES
}

pub(super) const fn default_history_retention_days() -> u32 {
    rd_core::DEFAULT_HISTORY_RETENTION_DAYS
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
            history_retention_entries: default_history_retention_entries(),
            history_retention_days: default_history_retention_days(),
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
            direct_unpack: false,
            sfv_verify: true,
            safe_postproc: true,
            delete_par2: false,
            enable_all_par: false,
            fail_hopeless_jobs: true,
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

pub(super) fn default_upload_mode() -> String {
    "copy".to_owned()
}

pub(super) fn default_gallery_max_parallel() -> u32 {
    2
}

pub(super) const fn default_remote_max_parallel() -> u32 {
    2
}

pub(super) const fn default_remote_timeout() -> u32 {
    60
}

pub(super) fn default_record_quality() -> String {
    "best".to_owned()
}

pub(super) fn default_record_poll_interval() -> u32 {
    120
}

pub(super) fn default_record_max_parallel() -> u32 {
    2
}

pub(super) fn default_torrent_seed_ratio() -> f64 {
    1.0
}

pub(super) const fn default_completion_countdown() -> u32 {
    rd_power::DEFAULT_COMPLETION_COUNTDOWN
}

pub(super) fn default_bandwidth_timezone() -> String {
    rd_core::DEFAULT_BANDWIDTH_TIMEZONE.to_owned()
}

pub(super) fn default_storage_minimum_free_bytes() -> rd_core::ByteCount {
    rd_core::StorageSettings::default().storage_minimum_free_bytes
}

pub(super) fn default_storage_auto_resume() -> bool {
    rd_core::StorageSettings::default().storage_auto_resume
}

pub(super) fn default_byte_display() -> String {
    // Binary keeps the figures the previous versions showed; switching the default would make
    // every size in every installation change overnight for no reason the user asked for.
    "binary".to_owned()
}

pub(super) fn default_byte_unit() -> String {
    // Scaling every value on its own is what the interface has always done; pinning a unit is
    // the deliberate choice of somebody who wants a column to compare.
    "auto".to_owned()
}

pub(super) fn default_storage_unknown_size_headroom() -> u32 {
    rd_core::DEFAULT_UNKNOWN_SIZE_HEADROOM
}

pub(super) fn default_torrent_seeding_enabled() -> bool {
    false
}

/// On: fetching the same bytes twice is never what somebody wanted, and a mirror that is not
/// needed costs nothing where it waits.
pub(super) const fn default_mirror_detection() -> bool {
    true
}

/// Ten minutes: a router needs a minute or two to come back, and reconnecting in a tight loop
/// against a hoster that is simply refusing gains nothing.
pub(super) const fn default_reconnect_interval_minutes() -> u32 {
    10
}

/// Three minutes covers a router reboot; past that something else is wrong.
pub(super) const fn default_reconnect_timeout_seconds() -> u32 {
    180
}

pub(super) const fn default_session_idle_hours() -> u32 {
    rd_core::DEFAULT_SESSION_IDLE_HOURS
}

pub(super) const fn default_session_max_hours() -> u32 {
    rd_core::DEFAULT_SESSION_MAX_HOURS
}

/// A day: long enough to notice a finished package, short enough to keep the queue readable.
pub(super) const fn default_auto_remove_delay_hours() -> u32 {
    24
}

pub(super) const fn default_auto_retry_interval_hours() -> u32 {
    rd_scheduler::DEFAULT_AUTO_RETRY_INTERVAL_HOURS
}

pub(super) const fn default_auto_retry_max_rounds() -> u32 {
    rd_scheduler::DEFAULT_AUTO_RETRY_MAX_ROUNDS
}

pub(super) const fn default_log_retention_records() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_RECORDS
}

pub(super) const fn default_log_retention_days() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_DAYS
}

pub(super) const fn default_audit_retention_records() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_RECORDS
}

pub(super) const fn default_audit_retention_days() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_DAYS
}

pub(super) const fn default_otlp_timeout_seconds() -> u32 {
    rd_core::DEFAULT_OTLP_TIMEOUT_SECONDS
}

/// Errors are worth looking at, so a package holding one is kept until it is dealt with.
pub(super) const fn default_auto_remove_keep_failed() -> bool {
    true
}

pub(super) fn default_keep_import_history() -> bool {
    true
}

/// A settings document written before the per-host limit existed gets the default rather
/// than an unbounded zero.
pub(super) const fn default_connections_per_host() -> u32 {
    rd_http::DEFAULT_CONNECTIONS_PER_HOST as u32
}
