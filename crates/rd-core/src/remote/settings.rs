//! Provider names for remote links and the tunables of the remote transports (part of the
//! `service.settings` blob, keys prefixed `remote_`).

use serde::{Deserialize, Serialize};

/// Provider name stored on FTP/FTPS candidates.
pub const FTP_PROVIDER: &str = "ftp";
/// Provider name stored on SFTP candidates.
pub const SFTP_PROVIDER: &str = "sftp";
/// Provider name stored on WebDAV candidates. WebDAV downloads still run as
/// `DownloadKind::Http`; the provider only marks how the link was resolved.
pub const WEBDAV_PROVIDER: &str = "webdav";

/// Runtime limits for the FTP and SFTP runners.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RemoteSettings {
    /// Concurrent FTP/SFTP transfers (1–8). Servers commonly cap simultaneous logins per
    /// account, so this stays low by default.
    pub remote_max_parallel: u32,
    /// Connect, login and per-read timeout in seconds (5–600).
    pub remote_timeout_seconds: u32,
    /// Whether an unknown SSH host key may be trusted on first use without asking.
    /// Off by default and deliberately hard to reach: silent TOFU is exactly the failure
    /// mode the host-key store exists to prevent.
    pub remote_ssh_auto_trust: bool,
}

impl RemoteSettings {
    /// Clamps stored values into their supported ranges.
    #[must_use]
    pub const fn sanitized(self) -> Self {
        Self {
            remote_max_parallel: clamp(self.remote_max_parallel, 1, 8),
            remote_timeout_seconds: clamp(self.remote_timeout_seconds, 5, 600),
            remote_ssh_auto_trust: self.remote_ssh_auto_trust,
        }
    }

    /// Timeout as a duration.
    #[must_use]
    pub const fn timeout(self) -> std::time::Duration {
        std::time::Duration::from_secs(self.remote_timeout_seconds as u64)
    }
}

const fn clamp(value: u32, low: u32, high: u32) -> u32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            remote_max_parallel: 2,
            remote_timeout_seconds: 60,
            remote_ssh_auto_trust: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteSettings;

    #[test]
    fn out_of_range_values_are_clamped() {
        let settings = RemoteSettings {
            remote_max_parallel: 99,
            remote_timeout_seconds: 1,
            remote_ssh_auto_trust: false,
        }
        .sanitized();
        assert_eq!(settings.remote_max_parallel, 8);
        assert_eq!(settings.remote_timeout_seconds, 5);
    }

    #[test]
    fn host_key_trust_is_off_by_default() {
        assert!(!RemoteSettings::default().remote_ssh_auto_trust);
    }
}
