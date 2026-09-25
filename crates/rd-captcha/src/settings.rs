//! Persisted captcha configuration (settings key `captcha`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Settings key holding [`CaptchaSettings`].
pub const SETTINGS_KEY: &str = "captcha";

/// Default endpoint of the solver service; every 2captcha-compatible API (CapMonster,
/// CapSolver, …) exposes the same `createTask`/`getTaskResult` pair on its own host.
pub const DEFAULT_ENDPOINT: &str = "https://api.2captcha.com";

/// How captchas are solved.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SolverKind {
    /// No solver service; only manual solving (image captchas) remains.
    #[default]
    None,
    /// Any service speaking the 2captcha `createTask`/`getTaskResult` JSON API.
    TwoCaptchaCompatible,
}

/// Captcha configuration as stored in the settings table.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptchaSettings {
    #[serde(default)]
    pub solver: SolverKind,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// Reference of the solver API key in the secret store; never the key itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    #[serde(default = "default_manual_enabled")]
    pub manual_enabled: bool,
    #[serde(default = "default_manual_timeout")]
    pub manual_timeout_seconds: u64,
}

fn default_endpoint() -> String {
    DEFAULT_ENDPOINT.to_owned()
}

const fn default_manual_enabled() -> bool {
    true
}

/// Long enough to notice the prompt and type an image captcha, short enough that an
/// unattended instance releases the download slot again.
const fn default_manual_timeout() -> u64 {
    180
}

impl Default for CaptchaSettings {
    fn default() -> Self {
        Self {
            solver: SolverKind::None,
            endpoint: default_endpoint(),
            api_key_ref: None,
            manual_enabled: default_manual_enabled(),
            manual_timeout_seconds: default_manual_timeout(),
        }
    }
}

impl CaptchaSettings {
    /// Whether a solver service is configured well enough to be attempted.
    #[must_use]
    pub fn has_solver(&self) -> bool {
        self.solver != SolverKind::None && self.api_key_ref.is_some()
    }

    /// Clamps stored values that a hand-edited settings row could put out of range.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.manual_timeout_seconds = self.manual_timeout_seconds.clamp(15, 600);
        if self.endpoint.trim().is_empty() {
            self.endpoint = default_endpoint();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptchaSettings, SolverKind};

    #[test]
    fn an_empty_stored_object_yields_working_defaults() {
        let settings: CaptchaSettings = serde_json::from_str("{}").expect("defaults");
        assert_eq!(settings.solver, SolverKind::None);
        assert!(settings.manual_enabled);
        assert!(!settings.has_solver());
        assert_eq!(settings.endpoint, super::DEFAULT_ENDPOINT);
    }

    #[test]
    fn a_solver_counts_as_configured_only_with_a_key() {
        let mut settings = CaptchaSettings {
            solver: SolverKind::TwoCaptchaCompatible,
            ..CaptchaSettings::default()
        };
        assert!(!settings.has_solver(), "a solver without a key is unusable");
        settings.api_key_ref = Some("captcha_solver_key".to_owned());
        assert!(settings.has_solver());
    }

    #[test]
    fn out_of_range_timeouts_are_clamped() {
        let settings = CaptchaSettings {
            manual_timeout_seconds: 100_000,
            endpoint: "  ".to_owned(),
            ..CaptchaSettings::default()
        }
        .sanitized();
        assert_eq!(settings.manual_timeout_seconds, 600);
        assert_eq!(settings.endpoint, super::DEFAULT_ENDPOINT);
    }
}
