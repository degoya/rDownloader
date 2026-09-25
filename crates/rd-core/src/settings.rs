use serde::{Deserialize, Serialize};

use crate::{ByteCount, PostprocessLevel};

/// Archive/postprocessing settings shared by the API, the extraction service and workers.
///
/// Stored as part of the `service.settings` JSON blob; unknown keys are ignored so every
/// consumer can deserialize the same value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PostprocessSettings {
    /// Deprecated: superseded by `default_level` (kept so old blobs still map).
    pub auto_extract: bool,
    /// Deprecated: superseded by `default_level`.
    pub delete_archives_after_extract: bool,
    /// Post-processing level for packages without an explicit/category level.
    pub default_level: Option<PostprocessLevel>,
    /// Keep NZB import entries and stored `.torrent` files after the download finishes.
    pub keep_import_history: bool,
    /// Stop dispatching new downloads while a package is post-processing.
    pub pause_during_postprocess: bool,
    /// Lower-case extensions (without dot) deleted from the package folder after unpacking.
    pub cleanup_extensions: Vec<String>,
    /// Delete sample files after unpacking and skip sample archives.
    pub ignore_samples: bool,
    /// Also extract archives found inside extracted archives (depth-capped).
    pub recursive_unpack: bool,
    /// Verify the CRC32 checksums of any `.sfv` index found in the package before unpacking.
    pub sfv_verify: bool,
    /// Whether a failed verification blocks everything after it.
    ///
    /// SABnzbd's `safe_postproc`, default on there and here: with it on, a package whose PAR2
    /// repair, SFV check or RAR test failed is not unpacked, tidied or handed to plugin steps,
    /// because what came out could be rubbish. With it off, those steps run anyway — a broken
    /// recovery set beside intact archives is a real case, and it used to leave the package
    /// untouchable (RD-104-04). The package action "post-process anyway" overrides it once,
    /// for one run, without changing the setting.
    pub safe_postproc: bool,
    /// Delete the PAR2 recovery set once unpacking has succeeded. Off by default: recovery
    /// data is the only thing that can rescue a damaged package, and a retry after a failed
    /// unpack still wants it.
    pub delete_par2: bool,
    /// Download every PAR2 recovery volume of an NZB straight away.
    ///
    /// SABnzbd's `enable_all_par`, off here as there: with it off the main index comes down
    /// with the payload and the `vol` volumes are held back, and only a repair that reports
    /// missing blocks fetches as many of them as the gap needs (RD-107-04). On restores the
    /// older behaviour, where every volume is fetched whether or not anything was damaged.
    pub enable_all_par: bool,
    /// Post-processing plugin steps enabled by default, by plugin id and in the order they
    /// run. Empty until somebody enables one: an installed step does nothing until it is
    /// switched on, so installing a plugin never changes what an existing package does.
    pub plugin_steps: Vec<String>,
    /// Files whose stem contains "sample" count as samples only below this size.
    pub sample_max_bytes: ByteCount,
    /// Absolute directory holding post-processing scripts; `None` = `scripts` next to the DB.
    pub scripts_directory: Option<String>,
    /// Kill a script after this many seconds.
    pub script_timeout_seconds: u32,
    /// Absolute path of the password list; `None` = `passwords.txt` next to the database.
    pub passwords_file: Option<String>,
    pub archive_max_files: u32,
    pub archive_max_uncompressed_bytes: ByteCount,
    pub rar_executable: Option<String>,
    pub rar_tool: String,
    /// Directory searched for unrar/7z before `PATH`; `None` = the built-in vendor folders.
    pub vendor_directory: Option<String>,
    /// Upload finished packages to an rclone remote after the other steps.
    pub upload_enabled: bool,
    /// rclone target in `remote:path` form; the package folder is created below it.
    pub upload_remote: Option<String>,
    /// `copy` keeps the local files, `move` removes them after a successful upload.
    pub upload_mode: String,
    pub rclone_executable: Option<String>,
}

impl PostprocessSettings {
    /// Effective global level: the explicit `default_level`, else derived from the
    /// deprecated switches of older settings blobs.
    #[must_use]
    pub fn effective_default_level(&self) -> PostprocessLevel {
        self.default_level
            .unwrap_or(if self.delete_archives_after_extract {
                PostprocessLevel::Delete
            } else {
                PostprocessLevel::Unpack
            })
    }

    /// Default cleanup list: index/checksum leftovers nobody keeps.
    #[must_use]
    pub fn default_cleanup_extensions() -> Vec<String> {
        ["nfo", "sfv", "srr", "url", "nzb"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

impl Default for PostprocessSettings {
    fn default() -> Self {
        Self {
            auto_extract: false,
            delete_archives_after_extract: false,
            default_level: None,
            keep_import_history: true,
            pause_during_postprocess: true,
            cleanup_extensions: Self::default_cleanup_extensions(),
            ignore_samples: true,
            recursive_unpack: false,
            sfv_verify: true,
            safe_postproc: true,
            delete_par2: false,
            enable_all_par: false,
            plugin_steps: Vec::new(),
            sample_max_bytes: ByteCount::new(300 * 1024 * 1024).expect("sample limit fits"),
            scripts_directory: None,
            script_timeout_seconds: 3600,
            passwords_file: None,
            archive_max_files: 20_000,
            archive_max_uncompressed_bytes: ByteCount::new(100 * 1024 * 1024 * 1024)
                .expect("default archive limit fits SQLite"),
            rar_executable: None,
            rar_tool: "unrar".to_owned(),
            vendor_directory: None,
            upload_enabled: false,
            upload_remote: None,
            upload_mode: "copy".to_owned(),
            rclone_executable: None,
        }
    }
}

/// Which transfer services are switched on.
///
/// Lives here because both the REST settings document and the service's own startup path map
/// the same six switches onto transfer kinds, and two copies of that mapping would drift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceSwitches {
    pub torrent: bool,
    pub usenet: bool,
    pub media: bool,
    pub gallery: bool,
    pub recording: bool,
    /// FTP, FTPS and SFTP together. They are one thing to the person using them — remote file
    /// transfer — and splitting the control three ways would be a distinction only the code
    /// cares about.
    pub remote: bool,
}

impl Default for ServiceSwitches {
    fn default() -> Self {
        Self {
            torrent: true,
            usenet: true,
            media: true,
            gallery: true,
            recording: true,
            remote: true,
        }
    }
}

impl ServiceSwitches {
    /// The transfer kinds that are switched off.
    ///
    /// WebDAV has no kind of its own — it rides the HTTP engine — so it cannot be refused
    /// here; intake turns it away instead.
    #[must_use]
    pub fn disabled_kinds(&self) -> Vec<crate::DownloadKind> {
        use crate::DownloadKind;

        let mut disabled = Vec::new();
        for (on, kinds) in [
            (self.torrent, &[DownloadKind::Torrent][..]),
            (self.usenet, &[DownloadKind::Usenet][..]),
            (self.media, &[DownloadKind::Media][..]),
            (self.gallery, &[DownloadKind::Gallery][..]),
            (self.recording, &[DownloadKind::Record][..]),
            (self.remote, &[DownloadKind::Ftp, DownloadKind::Sftp][..]),
        ] {
            if !on {
                disabled.extend_from_slice(kinds);
            }
        }
        disabled
    }
}

#[cfg(test)]
mod tests {
    use super::{PostprocessLevel, PostprocessSettings, ServiceSwitches};

    #[test]
    fn legacy_switches_map_to_a_level() {
        let legacy: PostprocessSettings =
            serde_json::from_str(r#"{"auto_extract":true,"delete_archives_after_extract":true}"#)
                .expect("legacy blob");
        assert_eq!(legacy.effective_default_level(), PostprocessLevel::Delete);
        let none: PostprocessSettings =
            serde_json::from_str(r#"{"default_level":null}"#).expect("explicit null");
        assert_eq!(none.effective_default_level(), PostprocessLevel::Unpack);
        let explicit: PostprocessSettings = serde_json::from_str(
            r#"{"default_level":"none","delete_archives_after_extract":true}"#,
        )
        .expect("explicit level");
        assert_eq!(explicit.effective_default_level(), PostprocessLevel::None);
        assert_eq!(
            PostprocessSettings::default().effective_default_level(),
            PostprocessLevel::Unpack
        );
    }

    #[test]
    fn import_history_is_kept_for_blobs_without_the_key() {
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(legacy.keep_import_history);
    }

    #[test]
    fn sfv_verification_is_on_for_blobs_without_the_key() {
        // Existing installations have no `sfv_verify` in their blob and gain the check.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(legacy.sfv_verify);
        let disabled: PostprocessSettings =
            serde_json::from_str(r#"{"sfv_verify":false}"#).expect("explicit opt-out");
        assert!(!disabled.sfv_verify);
    }

    #[test]
    fn every_service_is_on_until_it_is_switched_off() {
        use crate::DownloadKind;

        assert!(ServiceSwitches::default().disabled_kinds().is_empty());
        let off = ServiceSwitches {
            torrent: false,
            remote: false,
            ..ServiceSwitches::default()
        };
        // One switch covers FTP and SFTP, because they are one service to the person using
        // them; WebDAV has no kind and is refused at intake instead.
        assert_eq!(
            off.disabled_kinds(),
            vec![DownloadKind::Torrent, DownloadKind::Ftp, DownloadKind::Sftp]
        );
    }

    #[test]
    fn a_failed_repair_keeps_blocking_the_unpack_unless_that_is_switched_off() {
        // SABnzbd's default and ours: an installation that never chose otherwise keeps the
        // careful behaviour, so switching this on cannot surprise anybody's existing setup.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(legacy.safe_postproc);
        let relaxed: PostprocessSettings =
            serde_json::from_str(r#"{"safe_postproc":false}"#).expect("explicit opt-out");
        assert!(!relaxed.safe_postproc);
    }

    #[test]
    fn recovery_volumes_are_postponed_unless_all_of_them_are_asked_for() {
        // Off by default, as in SABnzbd: a package that needs no repair should not pay for
        // recovery data it never reads.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.enable_all_par);
        let eager: PostprocessSettings =
            serde_json::from_str(r#"{"enable_all_par":true}"#).expect("explicit opt-in");
        assert!(eager.enable_all_par);
    }

    #[test]
    fn par2_deletion_stays_off_unless_it_is_asked_for() {
        // Recovery data is the only thing that can rescue a damaged package. An installation
        // that never chose to discard it must keep it.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.delete_par2);
        let enabled: PostprocessSettings =
            serde_json::from_str(r#"{"delete_par2":true}"#).expect("explicit opt-in");
        assert!(enabled.delete_par2);
    }
}
