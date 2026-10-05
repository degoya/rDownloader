//! Validation and normalisation of a settings update before it is stored.

use super::*;

mod postprocess;

impl SettingsResponse {
    fn validate_media(&mut self, max_path: usize) -> Result<(), crate::ApiError> {
        self.validate_media_tools(max_path)?;
        self.validate_media_downloads()?;
        self.validate_gallery_and_remote()?;
        self.validate_recording()?;
        self.validate_torrent()
    }

    /// The paths of the external media tools: absolute, or not set.
    fn validate_media_tools(&mut self, max_path: usize) -> Result<(), crate::ApiError> {
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
        Ok(())
    }

    /// Media hosts, the default variant, the output template and criteria, and the limits.
    fn validate_media_downloads(&mut self) -> Result<(), crate::ApiError> {
        self.media_hosts = self
            .media_hosts
            .iter()
            .map(|host| rd_core::host_key(host.trim().trim_end_matches('/')))
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
        Ok(())
    }

    /// Gallery hosts and limits, and the limits of FTP/SFTP transfers.
    fn validate_gallery_and_remote(&mut self) -> Result<(), crate::ApiError> {
        self.gallery_hosts = self
            .gallery_hosts
            .iter()
            .map(|host| rd_core::host_key(host.trim().trim_end_matches('/')))
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
        Ok(())
    }

    /// The defaults of stream recordings.
    fn validate_recording(&mut self) -> Result<(), crate::ApiError> {
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
        Ok(())
    }

    /// Seeding limits, the IP blocklist and the network interface of the torrent engine.
    fn validate_torrent(&mut self) -> Result<(), crate::ApiError> {
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

    /// RD-191-12: the automatic retry's interval and rounds, each with its own stable code.
    #[test]
    fn the_automatic_retry_settings_are_checked_before_they_are_stored() {
        let code = |settings: SettingsResponse| {
            let mut settings = settings;
            settings
                .validate_postprocess()
                .err()
                .map(|error| error.code().to_owned())
        };
        let defaults = SettingsResponse::default();
        assert!(!defaults.auto_retry_failed);
        assert_eq!(defaults.auto_retry_interval_hours, 6);
        assert_eq!(defaults.auto_retry_max_rounds, 3);
        assert_eq!(code(defaults), None);
        for (hours, expected) in [
            (0, Some("settings.auto_retry_interval_invalid")),
            (1, None),
            (24, None),
            (25, Some("settings.auto_retry_interval_invalid")),
        ] {
            let settings = SettingsResponse {
                auto_retry_failed: true,
                auto_retry_interval_hours: hours,
                ..SettingsResponse::default()
            };
            assert_eq!(code(settings).as_deref(), expected, "{hours} hours");
        }
        for (rounds, expected) in [
            (0, None),
            (100, None),
            (101, Some("settings.auto_retry_rounds_too_high")),
        ] {
            let settings = SettingsResponse {
                auto_retry_max_rounds: rounds,
                ..SettingsResponse::default()
            };
            assert_eq!(code(settings).as_deref(), expected, "{rounds} rounds");
        }
    }

    /// RA-IN-06: media and gallery hosts are stored in `rd_core::host_key`'s form.
    #[test]
    fn media_and_gallery_hosts_are_stored_as_host_keys() {
        let mut settings = SettingsResponse {
            media_hosts: vec!["WWW.Example.COM./".to_owned(), "  ".to_owned()],
            gallery_hosts: vec!["www.Pixiv.net".to_owned()],
            ..SettingsResponse::default()
        };
        settings.validate_media(4096).expect("valid hosts");
        assert_eq!(settings.media_hosts, ["example.com"]);
        assert_eq!(settings.gallery_hosts, ["pixiv.net"]);
    }
}
