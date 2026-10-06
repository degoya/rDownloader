//! The post-processing half of a settings update: paths, limits, retries, archives and tools.

use super::*;
use crate::input_checks::optional_text;

/// Longest path or address a post-processing setting may hold.
const MAX_PATH_LENGTH: usize = 4096;

impl SettingsResponse {
    /// Normalises the post-processing paths and rejects values outside the supported ranges.
    pub fn validate_postprocess(&mut self) -> Result<(), crate::ApiError> {
        const MIN_AUTO_REMOVE_DELAY_HOURS: u32 = 1;
        // A month. Beyond that the setting is indistinguishable from leaving it switched off.
        const MAX_AUTO_REMOVE_DELAY_HOURS: u32 = 720;
        self.validate_media(MAX_PATH_LENGTH)?;
        self.validate_cleanup_and_scripts()?;
        self.validate_upload()?;
        self.validate_malware_scan()?;
        self.validate_retries_and_reconnect()?;
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
        self.validate_archives()?;
        self.validate_paths_and_tools()?;
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

    /// Cleanup extensions, the sample limit, and the scripts directory and timeout.
    fn validate_cleanup_and_scripts(&mut self) -> Result<(), crate::ApiError> {
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
        self.scripts_directory = optional_text(self.scripts_directory.take());
        if self.scripts_directory.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.scripts_directory_invalid",
                format!("Scripts directory must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        Ok(())
    }

    /// The upload after post-processing: its mode, its rclone target and the rclone binary.
    fn validate_upload(&mut self) -> Result<(), crate::ApiError> {
        if !matches!(self.upload_mode.as_str(), "copy" | "move") {
            return Err(crate::ApiError::bad_request(
                "settings.upload_mode_invalid",
                "Upload mode must be 'copy' or 'move'",
            ));
        }
        self.upload_remote = normalize_upload_remote(self.upload_remote.take())?;
        self.rclone_executable = optional_text(self.rclone_executable.take());
        if self.rclone_executable.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.rclone_path_invalid",
                format!("rclone must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        Ok(())
    }

    /// The retries of a failed file, the automatic retry and the reconnect.
    fn validate_retries_and_reconnect(&mut self) -> Result<(), crate::ApiError> {
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
        if !(rd_scheduler::MIN_AUTO_RETRY_INTERVAL_HOURS
            ..=rd_scheduler::MAX_AUTO_RETRY_INTERVAL_HOURS)
            .contains(&self.auto_retry_interval_hours)
        {
            return Err(crate::ApiError::bad_request(
                "settings.auto_retry_interval_invalid",
                format!(
                    "The interval of the automatic retry must be between {} and {} hours",
                    rd_scheduler::MIN_AUTO_RETRY_INTERVAL_HOURS,
                    rd_scheduler::MAX_AUTO_RETRY_INTERVAL_HOURS
                ),
            )
            .with_param("min", rd_scheduler::MIN_AUTO_RETRY_INTERVAL_HOURS)
            .with_param("max", rd_scheduler::MAX_AUTO_RETRY_INTERVAL_HOURS));
        }
        if self.auto_retry_max_rounds > rd_scheduler::MAX_AUTO_RETRY_ROUNDS {
            return Err(crate::ApiError::bad_request(
                "settings.auto_retry_rounds_too_high",
                format!(
                    "The automatic retry may run at most {} rounds per download",
                    rd_scheduler::MAX_AUTO_RETRY_ROUNDS
                ),
            )
            .with_param("max", rd_scheduler::MAX_AUTO_RETRY_ROUNDS));
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
        Ok(())
    }

    /// Archive limits, the RAR tool and the password list.
    fn validate_archives(&mut self) -> Result<(), crate::ApiError> {
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
        self.rar_executable = optional_text(self.rar_executable.take());
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
        self.passwords_file = optional_text(self.passwords_file.take());
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
        Ok(())
    }

    /// The vendor directory, the domain blocklist file, the tool manifest and the tool
    /// compatibility overrides.
    fn validate_paths_and_tools(&mut self) -> Result<(), crate::ApiError> {
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
            *value = optional_text(value.take());
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
        self.managed_tools_manifest_url = optional_text(self.managed_tools_manifest_url.take());
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
        Ok(())
    }
}
