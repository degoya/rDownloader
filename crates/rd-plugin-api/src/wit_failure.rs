//! The WIT-shaped failure variant and its conversions to and from `rd_core::FailureKind`.

use rd_core::FailureKind;
use serde::{Deserialize, Serialize};

/// WIT-shaped failure variant used by adapters and parity tests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WitFailureKind {
    Transient { retry_after_seconds: Option<u64> },
    Permanent,
    Offline,
    AuthRequired,
    AccountInvalid,
    RateLimited { retry_after_seconds: Option<u64> },
    NeedsCaptcha,
    Unsupported,
    IpBlocked { retry_after_seconds: Option<u64> },
    CaptchaFailed,
}

impl From<FailureKind> for WitFailureKind {
    fn from(value: FailureKind) -> Self {
        match value {
            FailureKind::Transient {
                retry_after_seconds,
            } => Self::Transient {
                retry_after_seconds,
            },
            FailureKind::Permanent => Self::Permanent,
            FailureKind::Offline => Self::Offline,
            FailureKind::AuthRequired => Self::AuthRequired,
            FailureKind::AccountInvalid => Self::AccountInvalid,
            FailureKind::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            FailureKind::NeedsCaptcha => Self::NeedsCaptcha,
            FailureKind::Unsupported => Self::Unsupported,
            FailureKind::IpBlocked {
                retry_after_seconds,
            } => Self::IpBlocked {
                retry_after_seconds,
            },
            FailureKind::CaptchaFailed => Self::CaptchaFailed,
        }
    }
}

impl From<WitFailureKind> for FailureKind {
    fn from(value: WitFailureKind) -> Self {
        match value {
            WitFailureKind::Transient {
                retry_after_seconds,
            } => Self::Transient {
                retry_after_seconds,
            },
            WitFailureKind::Permanent => Self::Permanent,
            WitFailureKind::Offline => Self::Offline,
            WitFailureKind::AuthRequired => Self::AuthRequired,
            WitFailureKind::AccountInvalid => Self::AccountInvalid,
            WitFailureKind::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            WitFailureKind::NeedsCaptcha => Self::NeedsCaptcha,
            WitFailureKind::Unsupported => Self::Unsupported,
            WitFailureKind::IpBlocked {
                retry_after_seconds,
            } => Self::IpBlocked {
                retry_after_seconds,
            },
            WitFailureKind::CaptchaFailed => Self::CaptchaFailed,
        }
    }
}
