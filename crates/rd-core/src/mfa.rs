//! Second-factor credentials, as the owner sees them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::MfaCredentialId;

/// What kind of second factor a credential is.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MfaKind {
    /// An authenticator app holding a shared secret.
    Totp,
    /// A passkey: a key pair held by an authenticator and bound to this installation's origin.
    Webauthn,
}

impl MfaKind {
    /// The stored discriminator.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Webauthn => "webauthn",
        }
    }

    /// Reads the stored discriminator back.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "totp" => Some(Self::Totp),
            "webauthn" => Some(Self::Webauthn),
            _ => None,
        }
    }
}

/// One enrolled factor. Never carries the secret behind it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct MfaCredential {
    pub id: MfaCredentialId,
    pub kind: MfaKind,
    pub label: String,
    pub created_at: DateTime<Utc>,
    /// When the first correct code proved the enrolment works.
    ///
    /// An unconfirmed credential does not gate sign-in. Someone who starts enrolling and then
    /// cannot scan the code must not thereby be locked out of their own service.
    pub confirmed_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// How much of the second factor is set up, for the settings page and the login screen.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct MfaStatus {
    /// Whether at least one confirmed credential exists, so sign-in requires a second step.
    pub enabled: bool,
    pub credentials: Vec<MfaCredential>,
    /// How many recovery codes are still unspent.
    ///
    /// Shown because running out is a problem that only becomes visible at the worst moment.
    pub recovery_codes_remaining: u32,
}
