use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

/// Stable error classes shared by scheduler, plugins and API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FailureKind {
    Transient {
        retry_after_seconds: Option<u64>,
    },
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited {
        retry_after_seconds: Option<u64>,
    },
    NeedsCaptcha,
    Unsupported,
    /// This IP may not start another free download from the hoster yet. Retryable, but the
    /// scheduler additionally holds back every other link of the same hoster until it
    /// expires instead of burning waits and captchas on them.
    IpBlocked {
        retry_after_seconds: Option<u64>,
    },
    /// A captcha answer was rejected by the hoster; worth one more attempt with a fresh
    /// challenge, unlike [`Self::NeedsCaptcha`], which means none could be obtained.
    CaptchaFailed,
}

impl FailureKind {
    /// Whether the scheduler may retry this class automatically.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transient { .. }
                | Self::Offline
                | Self::RateLimited { .. }
                | Self::IpBlocked { .. }
                | Self::CaptchaFailed
        )
    }

    /// Optional delay supplied by the remote side.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Transient {
                retry_after_seconds,
            }
            | Self::RateLimited {
                retry_after_seconds,
            }
            | Self::IpBlocked {
                retry_after_seconds,
            } => retry_after_seconds.map(Duration::from_secs),
            _ => None,
        }
    }
}

/// Flat, string-only parameters attached to a coded message (counts, names, hosts).
pub type MessageParams = BTreeMap<String, String>;

/// Structured failure with a redaction-safe English message and an optional stable
/// code (`<domain>.<subject>_<condition>`) that clients translate.
#[derive(Clone, Debug, Deserialize, Error, Serialize, ToSchema)]
#[error("{message}")]
pub struct Failure {
    pub category: FailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: MessageParams,
}

impl Failure {
    /// Creates a structured failure without a translation code.
    #[must_use]
    pub fn new(category: FailureKind, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            code: None,
            params: BTreeMap::new(),
        }
    }

    /// Creates a structured failure carrying a stable translation code.
    #[must_use]
    pub fn coded(category: FailureKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            code: Some(code.to_owned()),
            params: BTreeMap::new(),
        }
    }

    /// Attaches a translation parameter.
    #[must_use]
    pub fn with_param(mut self, key: &str, value: impl ToString) -> Self {
        self.params.insert(key.to_owned(), value.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, FailureKind};

    #[test]
    fn legacy_failure_json_without_code_still_deserializes() {
        let legacy = r#"{"category":{"kind":"permanent"},"message":"old text"}"#;
        let failure: Failure = serde_json::from_str(legacy).expect("legacy failure");
        assert_eq!(failure.code, None);
        assert!(failure.params.is_empty());
        let coded = Failure::coded(FailureKind::Offline, "download.offline", "Offline")
            .with_param("host", "example.test");
        let json = serde_json::to_string(&coded).expect("json");
        assert!(json.contains("\"code\":\"download.offline\""));
        assert!(json.contains("\"host\":\"example.test\""));
    }
}
