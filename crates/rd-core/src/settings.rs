use serde::{Deserialize, Serialize};

use crate::{ByteCount, PostprocessLevel};

/// Archive/postprocessing settings shared by the API, the extraction service and workers.
///
/// Stored as part of the `service.settings` JSON blob; unknown keys are ignored so every
/// consumer can deserialize the same value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PostprocessSettings {
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
    /// Unpack every archive set into a folder of its own below the package folder, named after
    /// the archive (`Film.part1.rar` → `Film/`), instead of straight into the package folder.
    /// Off by default (RD-170-16).
    pub unpack_to_subfolder: bool,
    /// When the package folder holds nothing but one folder named like the package (a release
    /// whose archive carries its own folder, `Release/Release/…`), move that folder's content up
    /// one level and remove it; with `unpack_to_subfolder`, the same for every archive's folder
    /// (`X/X/…`). Nothing is ever overwritten. Off by default (RD-1140-01).
    pub unwrap_package_folder: bool,
    /// Unpack a Usenet package's multi-volume RAR set while the package is still downloading,
    /// volume by volume as each one arrives intact (SABnzbd's direct unpack). Off by default; a
    /// repair or a damaged volume discards the attempt and the set is unpacked afterwards as
    /// usual (RD-1100-07).
    pub direct_unpack: bool,
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
    /// Stop a Usenet download as soon as it is known to be beyond repair (RD-1100-02).
    ///
    /// SABnzbd's `fail_hopeless_jobs`, on here as there: once more PAR2 blocks are missing
    /// than the set's recovery volumes - fetched, waiting or postponed - can replace, the
    /// rest of the set is not downloaded and the package fails with
    /// `usenet.job_hopeless`. Off downloads every file to the end as before.
    pub fail_hopeless_jobs: bool,
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
    /// Scan every finished package with ClamAV before it counts as finished (RD-190-14).
    ///
    /// Off by default: it needs a `clamd` somebody runs. A finding fails the package and stops
    /// everything after the scan; a `clamd` that cannot be reached is a warning on the step and
    /// the package carries on (fail-open, the owner's decision of 2026-10-02).
    pub malware_scan_enabled: bool,
    /// Where `clamd` listens: `host:port` over TCP, or `unix:/path` (an absolute path alone
    /// works too) for its local socket; `None` = `127.0.0.1:3310`, clamd's own TCP default.
    pub clamd_address: Option<String>,
    /// Largest file streamed to `clamd`. A larger one is not scanned, and the step says how
    /// many were left out; clamd's own `StreamMaxLength` has to be at least this (its default
    /// is the 25 MiB used here).
    pub malware_scan_max_bytes: ByteCount,
    /// Seconds one exchange with `clamd` may take: the connection, a chunk, the verdict.
    pub malware_scan_timeout_seconds: u32,
}

impl PostprocessSettings {
    /// The `clamd` address in force: the setting, else clamd's own TCP default.
    #[must_use]
    pub fn effective_clamd_address(&self) -> &str {
        self.clamd_address
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .unwrap_or(DEFAULT_CLAMD_ADDRESS)
    }

    /// Effective global level: the explicit `default_level`, else unpacking.
    #[must_use]
    pub fn effective_default_level(&self) -> PostprocessLevel {
        self.default_level.unwrap_or(PostprocessLevel::Unpack)
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
            default_level: None,
            keep_import_history: true,
            pause_during_postprocess: true,
            cleanup_extensions: Self::default_cleanup_extensions(),
            ignore_samples: true,
            recursive_unpack: false,
            unpack_to_subfolder: false,
            unwrap_package_folder: false,
            direct_unpack: false,
            sfv_verify: true,
            safe_postproc: true,
            delete_par2: false,
            enable_all_par: false,
            fail_hopeless_jobs: true,
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
            malware_scan_enabled: false,
            clamd_address: None,
            malware_scan_max_bytes: ByteCount::new(DEFAULT_MALWARE_SCAN_MAX_BYTES)
                .expect("scan limit fits"),
            malware_scan_timeout_seconds: DEFAULT_MALWARE_SCAN_TIMEOUT_SECONDS,
        }
    }
}

/// clamd's own TCP default, used when no address is configured (RD-190-14).
pub const DEFAULT_CLAMD_ADDRESS: &str = "127.0.0.1:3310";
/// clamd's default `StreamMaxLength`, 25 MiB, so the two agree out of the box.
pub const DEFAULT_MALWARE_SCAN_MAX_BYTES: u64 = 25 * 1024 * 1024;
/// How long one exchange with clamd may take by default.
pub const DEFAULT_MALWARE_SCAN_TIMEOUT_SECONDS: u32 = 120;

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
    /// FTP, FTPS, SFTP and object storage together. They are one thing to the person using
    /// them — remote file transfer — and splitting the control four ways would be a
    /// distinction only the code cares about.
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
            (
                self.remote,
                &[
                    DownloadKind::Ftp,
                    DownloadKind::Sftp,
                    DownloadKind::ObjectStorage,
                ][..],
            ),
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
    fn an_unset_level_means_unpacking() {
        let none: PostprocessSettings =
            serde_json::from_str(r#"{"default_level":null}"#).expect("explicit null");
        assert_eq!(none.effective_default_level(), PostprocessLevel::Unpack);
        let explicit: PostprocessSettings =
            serde_json::from_str(r#"{"default_level":"none"}"#).expect("explicit level");
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
            vec![
                DownloadKind::Torrent,
                DownloadKind::Ftp,
                DownloadKind::Sftp,
                DownloadKind::ObjectStorage
            ]
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
    fn archives_unpack_into_the_package_folder_unless_a_folder_each_is_asked_for() {
        // RD-170-16: the folder per archive is opt-in; a blob that never mentions it keeps
        // unpacking straight into the package folder.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.unpack_to_subfolder);
        let folders: PostprocessSettings =
            serde_json::from_str(r#"{"unpack_to_subfolder":true}"#).expect("explicit opt-in");
        assert!(folders.unpack_to_subfolder);
    }

    #[test]
    fn a_folder_named_like_the_package_stays_unless_unwrapping_is_asked_for() {
        // RD-1140-01: opt-in; a blob that never mentions it keeps the folder where it is.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.unwrap_package_folder);
        let unwrap: PostprocessSettings =
            serde_json::from_str(r#"{"unwrap_package_folder":true}"#).expect("explicit opt-in");
        assert!(unwrap.unwrap_package_folder);
    }

    #[test]
    fn archives_are_unpacked_after_the_download_unless_direct_unpack_is_asked_for() {
        // RD-1100-07: opt-in; a blob that never mentions it unpacks once the package is complete.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.direct_unpack);
        let direct: PostprocessSettings =
            serde_json::from_str(r#"{"direct_unpack":true}"#).expect("explicit opt-in");
        assert!(direct.direct_unpack);
    }

    #[test]
    fn the_malware_scan_is_off_until_somebody_switches_it_on() {
        // RD-190-14: it needs a clamd somebody runs, so a blob that never mentions it scans
        // nothing, and the address falls back to clamd's own default.
        let legacy: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(!legacy.malware_scan_enabled);
        assert_eq!(legacy.effective_clamd_address(), "127.0.0.1:3310");
        assert_eq!(legacy.malware_scan_max_bytes.get(), 25 * 1024 * 1024);
        let on: PostprocessSettings = serde_json::from_str(
            r#"{"malware_scan_enabled":true,"clamd_address":"unix:/run/clamav/clamd.ctl"}"#,
        )
        .expect("explicit opt-in");
        assert!(on.malware_scan_enabled);
        assert_eq!(on.effective_clamd_address(), "unix:/run/clamav/clamd.ctl");
        let blank: PostprocessSettings =
            serde_json::from_str(r#"{"clamd_address":"  "}"#).expect("blank address");
        assert_eq!(blank.effective_clamd_address(), "127.0.0.1:3310");
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
    fn hopeless_usenet_jobs_are_given_up_unless_that_is_switched_off() {
        // On by default, as in SABnzbd: a set that cannot be repaired should not spend a
        // block account's volume on the rest of its files.
        let missing: PostprocessSettings = serde_json::from_str("{}").expect("empty blob");
        assert!(missing.fail_hopeless_jobs);
        let off: PostprocessSettings =
            serde_json::from_str(r#"{"fail_hopeless_jobs":false}"#).expect("explicit opt-out");
        assert!(!off.fail_hopeless_jobs);
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
