//! Validation and normalisation of a settings update before it is stored.

use super::*;

impl SettingsResponse {
    fn validate_media(&mut self, max_path: usize) -> Result<(), crate::ApiError> {
        for (field, value) in [
            ("yt-dlp", &mut self.media_ytdlp_executable),
            ("ffmpeg", &mut self.media_ffmpeg_executable),
            ("gallery-dl", &mut self.gallery_executable),
            ("streamlink", &mut self.record_streamlink_executable),
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            if value.as_ref().is_some_and(|text| {
                text.len() > max_path || !std::path::Path::new(text).is_absolute()
            }) {
                return Err(crate::ApiError::bad_request(
                    "settings.media_tool_path_invalid",
                    format!("{field} must be an absolute path of at most {max_path} characters"),
                )
                .with_param("tool", field)
                .with_param("max", max_path));
            }
        }
        self.media_hosts = self
            .media_hosts
            .iter()
            .map(|host| {
                host.trim()
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            })
            .filter(|host| !host.is_empty())
            .collect();
        if let Some(bad) = self.media_hosts.iter().find(|host| {
            host.len() > 253
                || host.contains('/')
                || host.contains(':')
                || !host.contains('.')
                || host
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.media_host_invalid",
                format!("Media host '{bad}' must be a bare domain name"),
            )
            .with_param("value", bad));
        }
        self.media_default_variant = self.media_default_variant.trim().to_owned();
        // `custom` accompanies an explicit criteria set; the preset ids stay valid so an
        // existing API caller keeps working unchanged.
        let valid_variant = matches!(
            self.media_default_variant.as_str(),
            "best" | "audio_mp3" | "custom"
        ) || self
            .media_default_variant
            .strip_suffix('p')
            .is_some_and(|digits| digits.parse::<u32>().is_ok());
        if !valid_variant {
            return Err(crate::ApiError::bad_request(
                "settings.media_variant_invalid",
                "Default media variant must be 'best', '<height>p', 'audio_mp3' or 'custom'",
            ));
        }
        self.media_output_template = self
            .media_output_template
            .take()
            .map(|template| template.trim().to_owned())
            .filter(|template| !template.is_empty());
        if let Some(template) = self.media_output_template.as_deref() {
            rd_files::validate(template).map_err(|error| {
                crate::ApiError::bad_request("media.template_invalid", error.to_string())
            })?;
        }
        if let Some(criteria) = self.media_default_criteria.take() {
            self.media_default_criteria = Some(criteria.sanitized().map_err(|error| {
                crate::ApiError::bad_request("media.criteria_invalid", error.to_string())
            })?);
        }
        if !(1..=8).contains(&self.media_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent media downloads must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(5..=600).contains(&self.media_check_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.media_timeout_invalid",
                "Media probe timeout must be between 5 and 600 seconds",
            )
            .with_param("min", 5)
            .with_param("max", 600));
        }
        self.gallery_hosts = self
            .gallery_hosts
            .iter()
            .map(|host| {
                host.trim()
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            })
            .filter(|host| !host.is_empty())
            .collect();
        if let Some(bad) = self.gallery_hosts.iter().find(|host| {
            host.len() > 253
                || host.contains('/')
                || host.contains(':')
                || !host.contains('.')
                || host
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.media_host_invalid",
                format!("Gallery host '{bad}' must be a bare domain name"),
            )
            .with_param("value", bad));
        }
        if !(1..=8).contains(&self.gallery_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent gallery downloads must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(1..=8).contains(&self.remote_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "remote.parallel_invalid",
                "Concurrent FTP/SFTP transfers must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(5..=600).contains(&self.remote_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "remote.timeout_invalid",
                "The FTP/SFTP timeout must be between 5 and 600 seconds",
            )
            .with_param("min", 5)
            .with_param("max", 600));
        }
        self.record_default_quality = self.record_default_quality.trim().to_owned();
        if self.record_default_quality.is_empty() || self.record_default_quality.len() > 50 {
            return Err(crate::ApiError::bad_request(
                "stream.quality_invalid",
                "Default stream quality must be 1-50 characters",
            ));
        }
        if !(60..=3600).contains(&self.record_poll_interval_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.record_poll_interval_invalid",
                "Channel poll interval must be between 60 and 3600 seconds",
            )
            .with_param("min", 60)
            .with_param("max", 3600));
        }
        if !(1..=8).contains(&self.record_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent recordings must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(0.0..=100.0).contains(&self.torrent_seed_ratio) || !self.torrent_seed_ratio.is_finite()
        {
            return Err(crate::ApiError::bad_request(
                "settings.torrent_seed_ratio_invalid",
                "Seed ratio must be between 0 and 100",
            )
            .with_param("min", 0)
            .with_param("max", 100));
        }
        if self
            .torrent_seed_time_minutes
            .is_some_and(|minutes| minutes == 0 || minutes > 60 * 24 * 365)
        {
            return Err(crate::ApiError::bad_request(
                "settings.torrent_seed_time_invalid",
                "Seed time limit must be between 1 minute and one year",
            ));
        }

        if let Some(url) = self.torrent_ip_blocklist_url.as_deref() {
            // The engine loads this itself; only an http(s) URL can ever work.
            let usable = url::Url::parse(url)
                .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"));
            if !usable {
                return Err(crate::ApiError::bad_request(
                    "torrent.blocklist_invalid",
                    "The IP blocklist must be an http or https URL",
                ));
            }
        }
        if let Some(interface) = self.torrent_bind_interface.as_deref()
            && !interface.trim().is_empty()
            && !rd_torrent::interfaces()
                .iter()
                .any(|candidate| candidate.name == interface)
        {
            return Err(crate::ApiError::bad_request(
                "torrent.interface_unknown",
                "The selected network interface does not exist",
            )
            .with_param("interface", interface.to_owned()));
        }
        Ok(())
    }
}

/// Normalises an rclone target: trimmed, `remote:path` shaped, no control characters.
pub fn normalize_upload_remote(value: Option<String>) -> Result<Option<String>, crate::ApiError> {
    let Some(trimmed) = value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let valid = trimmed.len() <= 1024
        && trimmed.contains(':')
        && !trimmed.starts_with(':')
        && !trimmed.chars().any(char::is_control);
    if !valid {
        return Err(crate::ApiError::bad_request(
            "settings.upload_remote_invalid",
            "Upload target must have the rclone form 'remote:path'",
        ));
    }
    Ok(Some(trimmed))
}

impl SettingsResponse {
    /// Normalises the post-processing paths and rejects values outside the supported ranges.
    pub fn validate_postprocess(&mut self) -> Result<(), crate::ApiError> {
        const MAX_PATH_LENGTH: usize = 4096;
        const MIN_AUTO_REMOVE_DELAY_HOURS: u32 = 1;
        // A month. Beyond that the setting is indistinguishable from leaving it switched off.
        const MAX_AUTO_REMOVE_DELAY_HOURS: u32 = 720;
        self.validate_media(MAX_PATH_LENGTH)?;
        self.cleanup_extensions = self
            .cleanup_extensions
            .iter()
            .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        if let Some(bad) = self
            .cleanup_extensions
            .iter()
            .find(|value| value.len() > 10 || !value.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err(crate::ApiError::bad_request(
                "settings.cleanup_extension_invalid",
                format!("Cleanup extension '{bad}' must be 1-10 alphanumeric characters"),
            )
            .with_param("value", bad));
        }
        if self.sample_max_bytes.get() == 0 {
            return Err(crate::ApiError::bad_request(
                "settings.sample_max_bytes_invalid",
                "Sample size limit must be greater than zero",
            ));
        }
        if !(10..=86_400).contains(&self.script_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.script_timeout_invalid",
                "Script timeout must be between 10 and 86400 seconds",
            )
            .with_param("min", 10)
            .with_param("max", 86_400));
        }
        self.scripts_directory = self
            .scripts_directory
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.scripts_directory.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.scripts_directory_invalid",
                format!("Scripts directory must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        if !matches!(self.upload_mode.as_str(), "copy" | "move") {
            return Err(crate::ApiError::bad_request(
                "settings.upload_mode_invalid",
                "Upload mode must be 'copy' or 'move'",
            ));
        }
        self.upload_remote = normalize_upload_remote(self.upload_remote.take())?;
        self.rclone_executable = self
            .rclone_executable
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.rclone_executable.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.rclone_path_invalid",
                format!("rclone must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        self.validate_malware_scan()?;
        if self.max_retries > rd_scheduler::MAX_CONFIGURABLE_RETRIES {
            return Err(crate::ApiError::bad_request(
                "settings.max_retries_too_high",
                format!(
                    "Retries per file must not exceed {}",
                    rd_scheduler::MAX_CONFIGURABLE_RETRIES
                ),
            )
            .with_param("max", rd_scheduler::MAX_CONFIGURABLE_RETRIES));
        }
        if !(1..=1440).contains(&self.reconnect_min_interval_minutes) {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_interval_invalid",
                "The gap between reconnects must be between 1 and 1440 minutes",
            ));
        }
        if !(30..=900).contains(&self.reconnect_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_timeout_invalid",
                "A reconnect must be given between 30 and 900 seconds",
            ));
        }
        for address in &self.reconnect_ip_check_urls {
            let parsed = url::Url::parse(address.trim());
            if !parsed.is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
                return Err(crate::ApiError::bad_request(
                    "settings.reconnect_url_invalid",
                    "An address check must be an http or https URL",
                )
                .with_param("value", address.clone()));
            }
        }
        // A reconnect without a script would hold the queue and then do nothing.
        if self.reconnect_enabled
            && self
                .reconnect_script
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
        {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_script_missing",
                "A reconnect needs the name of the script that reconnects",
            ));
        }
        if !(MIN_AUTO_REMOVE_DELAY_HOURS..=MAX_AUTO_REMOVE_DELAY_HOURS)
            .contains(&self.auto_remove_delay_hours)
        {
            return Err(crate::ApiError::bad_request(
                "settings.auto_remove_delay_invalid",
                format!(
                    "The delay before a finished package is removed must be between {MIN_AUTO_REMOVE_DELAY_HOURS} and {MAX_AUTO_REMOVE_DELAY_HOURS} hours"
                ),
            )
            .with_param("min", MIN_AUTO_REMOVE_DELAY_HOURS)
            .with_param("max", MAX_AUTO_REMOVE_DELAY_HOURS));
        }
        if !(1..=1_000_000).contains(&self.archive_max_files) {
            return Err(crate::ApiError::bad_request(
                "settings.archive_max_files_invalid",
                "Archive file limit must be between 1 and 1,000,000",
            )
            .with_param("min", 1)
            .with_param("max", 1_000_000));
        }
        if self.archive_max_uncompressed_bytes.get() == 0 {
            return Err(crate::ApiError::bad_request(
                "settings.archive_max_bytes_invalid",
                "Archive size limit must be greater than zero",
            ));
        }
        if !matches!(self.rar_tool.as_str(), "unrar" | "7z") {
            return Err(crate::ApiError::bad_request(
                "settings.rar_tool_invalid",
                "RAR tool must be either 'unrar' or '7z'",
            ));
        }
        self.rar_executable = self
            .rar_executable
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self
            .rar_executable
            .as_ref()
            .is_some_and(|value| value.len() > MAX_PATH_LENGTH)
        {
            return Err(crate::ApiError::bad_request(
                "settings.rar_executable_too_long",
                format!("RAR tool path must not exceed {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        if self
            .rar_executable
            .as_ref()
            .is_some_and(|value| !std::path::Path::new(value).is_absolute())
        {
            return Err(crate::ApiError::bad_request(
                "settings.rar_executable_not_absolute",
                "RAR tool must be given as an absolute path",
            ));
        }
        self.passwords_file = self
            .passwords_file
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.passwords_file.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.passwords_file_invalid",
                format!(
                    "Password list must be an absolute path of at most {MAX_PATH_LENGTH} characters"
                ),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        for (code, field, value) in [
            (
                "settings.vendor_directory_invalid",
                "Vendor directory",
                &mut self.vendor_directory,
            ),
            (
                "settings.excluded_domains_file_invalid",
                "Domain blocklist",
                &mut self.excluded_domains_file,
            ),
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            if value.as_ref().is_some_and(|text| {
                text.len() > MAX_PATH_LENGTH || !std::path::Path::new(text).is_absolute()
            }) {
                return Err(crate::ApiError::bad_request(
                    code,
                    format!(
                        "{field} must be an absolute path of at most {MAX_PATH_LENGTH} characters"
                    ),
                )
                .with_param("max", MAX_PATH_LENGTH));
            }
        }
        // A manifest served over plain HTTP is a manifest whoever sits on the path can
        // replace. The signature would still be checked, but refusing here says why rather
        // than failing later with "untrusted key" on a document nobody tampered with.
        self.managed_tools_manifest_url = self
            .managed_tools_manifest_url
            .take()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        if self
            .managed_tools_manifest_url
            .as_ref()
            .is_some_and(|url| url.len() > MAX_PATH_LENGTH || !url.starts_with("https://"))
        {
            return Err(crate::ApiError::bad_request(
                "settings.managed_tools_manifest_url_invalid",
                format!(
                    "The tool manifest URL must be an https address of at most                      {MAX_PATH_LENGTH} characters"
                ),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        // An override switches off a block for one named tool. A name nothing gates on would
        // look like it did something and do nothing, so it is refused rather than dropped.
        self.tool_compatibility_overrides = std::mem::take(&mut self.tool_compatibility_overrides)
            .into_iter()
            .map(|tool| tool.trim().to_lowercase())
            .filter(|tool| !tool.is_empty())
            .collect();
        self.tool_compatibility_overrides.sort();
        self.tool_compatibility_overrides.dedup();
        if let Some(unknown) = self
            .tool_compatibility_overrides
            .iter()
            .find(|tool| !rd_tools::compat::RULED_TOOLS.contains(&tool.as_str()))
        {
            return Err(crate::ApiError::bad_request(
                "settings.tool_compatibility_override_invalid",
                format!("{unknown} has no compatibility rule to override"),
            )
            .with_param("tool", unknown));
        }
        // Ports below 1024 need elevated privileges on Unix and would make the service fail
        // to start after a restart, i.e. lock the user out of the very UI they configured.
        if self.ui_port.is_some_and(|port| port < 1024) {
            return Err(crate::ApiError::bad_request(
                "settings.ui_port_invalid",
                "The UI port must be between 1024 and 65535",
            )
            .with_param("min", 1024)
            .with_param("max", 65_535));
        }
        Ok(())
    }
}

impl SettingsResponse {
    /// The ClamAV scan (RD-190-14): an address the client can read, a limit clamd can take, a
    /// timeout that is neither instant nor forever.
    fn validate_malware_scan(&mut self) -> Result<(), crate::ApiError> {
        /// clamd's `StreamMaxLength` is capped at 4 GiB.
        const MAX_SCAN_BYTES: u64 = 4 * 1024 * 1024 * 1024;
        self.clamd_address = self
            .clamd_address
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(address) = &self.clamd_address
            && let Err(error) = rd_extract::clamd::ClamdAddress::parse(address)
        {
            return Err(crate::ApiError::bad_request(
                "settings.clamd_address_invalid",
                format!("The clamd address must be host:port or unix:/path ({error})"),
            )
            .with_param("value", address.clone()));
        }
        if !(1..=MAX_SCAN_BYTES).contains(&self.malware_scan_max_bytes.get()) {
            return Err(crate::ApiError::bad_request(
                "settings.malware_scan_max_bytes_invalid",
                "The scan limit must be between 1 byte and 4 GiB",
            )
            .with_param("max", MAX_SCAN_BYTES));
        }
        if !(5..=3600).contains(&self.malware_scan_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.malware_scan_timeout_invalid",
                "The scan timeout must be between 5 and 3600 seconds",
            )
            .with_param("min", 5)
            .with_param("max", 3600));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsResponse;

    fn error_code(settings: &mut SettingsResponse) -> Option<String> {
        settings
            .validate_malware_scan()
            .err()
            .map(|error| error.code().to_owned())
    }

    #[test]
    fn the_malware_scan_settings_are_checked_before_they_are_stored() {
        let mut settings = SettingsResponse {
            clamd_address: Some("  clamav:3310 ".to_owned()),
            ..SettingsResponse::default()
        };
        assert_eq!(error_code(&mut settings), None);
        assert_eq!(settings.clamd_address.as_deref(), Some("clamav:3310"));
        // Blank is "the default", not an address.
        settings.clamd_address = Some("   ".to_owned());
        assert_eq!(error_code(&mut settings), None);
        assert_eq!(settings.clamd_address, None);
        settings.clamd_address = Some("http://scanner/".to_owned());
        assert_eq!(
            error_code(&mut settings).as_deref(),
            Some("settings.clamd_address_invalid")
        );
        settings.clamd_address = None;
        settings.malware_scan_timeout_seconds = 1;
        assert_eq!(
            error_code(&mut settings).as_deref(),
            Some("settings.malware_scan_timeout_invalid")
        );
    }
}
