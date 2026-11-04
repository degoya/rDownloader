//! Persisted captcha configuration (settings key `captcha`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Settings key holding [`CaptchaSettings`].
pub const SETTINGS_KEY: &str = "captcha";

/// Default endpoint of the solver service; every 2captcha-compatible API (CapMonster,
/// CapSolver, …) exposes the same `createTask`/`getTaskResult` pair on its own host.
pub const DEFAULT_ENDPOINT: &str = "https://api.2captcha.com";

/// The code of a solver endpoint over plain http while [`CaptchaSettings::allow_plain_http`]
/// is off (RD-1420-01): the API key would cross the network unencrypted.
pub const CODE_ENDPOINT_PLAIN_HTTP: &str = "captcha.endpoint_plain_http";

/// The code of a solver endpoint that is no absolute http(s) URL with a host.
pub const CODE_ENDPOINT_INVALID: &str = "captcha.endpoint_invalid";

/// Why a solver endpoint is not used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointRefusal {
    /// No absolute http(s) URL with a host.
    Invalid,
    /// Plain http without [`CaptchaSettings::allow_plain_http`].
    PlainHttp,
}

impl EndpointRefusal {
    /// The stable code of the refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => CODE_ENDPOINT_INVALID,
            Self::PlainHttp => CODE_ENDPOINT_PLAIN_HTTP,
        }
    }

    /// The refusal in English, for logs and API clients without a translation.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Invalid => "The solver endpoint must be an absolute https URL",
            Self::PlainHttp => {
                "The solver endpoint uses plain http, which would send the API key \
                 unencrypted: use https, or allow plain http in the captcha settings"
            }
        }
    }
}

/// The solver endpoint `value` as it is stored and used: trimmed and without a trailing slash,
/// the default when blank. Only an absolute https URL with a host, or an http one when
/// `allow_plain_http` — the endpoint receives the person's API key (RD-1420-01).
pub fn checked_endpoint(value: &str, allow_plain_http: bool) -> Result<String, EndpointRefusal> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_ENDPOINT.to_owned());
    }
    let url = url::Url::parse(trimmed).map_err(|_| EndpointRefusal::Invalid)?;
    if url.host_str().is_none_or(str::is_empty) {
        return Err(EndpointRefusal::Invalid);
    }
    match url.scheme() {
        "https" => {}
        "http" if allow_plain_http => {}
        "http" => return Err(EndpointRefusal::PlainHttp),
        _ => return Err(EndpointRefusal::Invalid),
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

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
    /// Whether the endpoint may be plain http, which sends the API key unencrypted — for a
    /// solver on this machine or in the person's own network (RD-1420-01). Off by default.
    #[serde(default)]
    pub allow_plain_http: bool,
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
            allow_plain_http: false,
        }
    }
}

impl CaptchaSettings {
    /// Whether a solver service is configured well enough to be attempted.
    #[must_use]
    pub(crate) fn has_solver(&self) -> bool {
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
    use super::{CaptchaSettings, EndpointRefusal, SolverKind, checked_endpoint};

    #[test]
    fn an_empty_stored_object_yields_working_defaults() {
        let settings: CaptchaSettings = serde_json::from_str("{}").expect("defaults");
        assert_eq!(settings.solver, SolverKind::None);
        assert!(settings.manual_enabled);
        assert!(!settings.has_solver());
        assert_eq!(settings.endpoint, super::DEFAULT_ENDPOINT);
        assert!(
            !settings.allow_plain_http,
            "plain http is off unless switched on"
        );
    }

    /// RD-1420-01 (audit L-08): the endpoint receives the API key, so plain http only with the
    /// setting, and never anything but an absolute http(s) URL.
    #[test]
    fn a_plain_http_endpoint_needs_the_setting() {
        assert_eq!(
            checked_endpoint("https://api.capmonster.cloud/", false),
            Ok("https://api.capmonster.cloud".to_owned())
        );
        assert_eq!(
            checked_endpoint("  ", false),
            Ok(super::DEFAULT_ENDPOINT.to_owned())
        );
        assert_eq!(
            checked_endpoint("http://127.0.0.1:8000/", false),
            Err(EndpointRefusal::PlainHttp)
        );
        assert_eq!(
            checked_endpoint("http://127.0.0.1:8000/", true),
            Ok("http://127.0.0.1:8000".to_owned())
        );
        assert_eq!(
            EndpointRefusal::PlainHttp.code(),
            "captcha.endpoint_plain_http"
        );
        for rejected in [
            "api.2captcha.com",
            "ftp://example.test",
            "https://",
            "http://",
        ] {
            assert_eq!(
                checked_endpoint(rejected, true),
                Err(EndpointRefusal::Invalid),
                "{rejected}"
            );
        }
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
