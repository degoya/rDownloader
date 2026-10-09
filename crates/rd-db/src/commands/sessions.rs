//! The commands of `writer/sessions.rs`.

use super::Reply;

/// The commands `Writer::handle_sessions` applies.
pub(crate) enum SessionsCommand {
    CreateCaptureToken {
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        calls_per_minute: Option<u32>,
        reply: Reply<rd_core::CaptureToken>,
    },
    UpdateCaptureTokenScopes {
        id: rd_core::CaptureTokenId,
        scopes: Vec<String>,
        reply: Reply<rd_core::CaptureToken>,
    },
    UpdateCaptureTokenLimits {
        id: rd_core::CaptureTokenId,
        calls_per_minute: Option<u32>,
        reply: Reply<rd_core::CaptureToken>,
    },
    RevokeCaptureToken {
        id: rd_core::CaptureTokenId,
        reply: Reply<()>,
    },
    CreateSession {
        id: rd_core::SessionId,
        token_sha256: String,
        user_agent: Option<String>,
        client_ip: Option<String>,
        lifetime_hours: i64,
        reply: Reply<rd_core::Session>,
    },
    TouchSession {
        token_sha256: String,
        reply: Reply<bool>,
    },
    RevokeSession {
        id: rd_core::SessionId,
        reply: Reply<bool>,
    },
    RevokeOtherSessions {
        keep_digest: String,
        reply: Reply<u64>,
    },
    RevokeAllSessions {
        reply: Reply<u64>,
    },
    PurgeExpiredSessions {
        limits: rd_core::SessionLimits,
        reply: Reply<u64>,
    },
    TouchCaptureToken {
        token_sha256: String,
        reply: Reply<()>,
    },
    CreateMfaCredential {
        id: rd_core::MfaCredentialId,
        kind: rd_core::MfaKind,
        label: String,
        material_ref: String,
        reply: Reply<rd_core::MfaCredential>,
    },
    ConfirmMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<bool>,
    },
    TouchMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<()>,
    },
    AcceptTotpStep {
        id: rd_core::MfaCredentialId,
        step: i64,
        reply: Reply<bool>,
    },
    RepointMfaMaterial {
        id: rd_core::MfaCredentialId,
        material_ref: String,
        reply: Reply<bool>,
    },
    DeleteMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<Option<String>>,
    },
    ReplaceRecoveryCodes {
        digests: Vec<String>,
        reply: Reply<()>,
    },
    SpendRecoveryCode {
        digest: String,
        reply: Reply<bool>,
    },
    ClearMfa {
        kind: rd_core::MfaKind,
        reply: Reply<Vec<String>>,
    },
}
