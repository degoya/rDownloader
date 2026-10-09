//! Database facade for capture tokens, sign-in sessions, MFA credentials and recovery codes.

use anyhow::Result;

use crate::{
    Database, capture_store,
    commands::{MaintenanceCommand, SessionsCommand},
    mfa_store, session_store, writer,
};

impl Database {
    /// Stores only the SHA-256 digest of a scoped bearer token that never expires.
    pub async fn create_capture_token(
        &self,
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
    ) -> Result<rd_core::CaptureToken> {
        self.create_expiring_capture_token(id, label, token_sha256, scopes, None)
            .await
    }

    /// [`Self::create_capture_token`] with an optional expiry (RD-1110-07): from `expires_at`
    /// on the digest matches no live token, exactly as after a revocation.
    pub async fn create_expiring_capture_token(
        &self,
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<rd_core::CaptureToken> {
        self.create_limited_capture_token(id, label, token_sha256, scopes, expires_at, None)
            .await
    }

    /// [`Self::create_expiring_capture_token`] with an optional call limit per minute
    /// (RD-1200-04); `None` is no limit.
    pub async fn create_limited_capture_token(
        &self,
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        calls_per_minute: Option<u32>,
    ) -> Result<rd_core::CaptureToken> {
        writer::request(&self.writer, |reply| SessionsCommand::CreateCaptureToken {
            id,
            label,
            token_sha256,
            scopes,
            expires_at,
            calls_per_minute,
            reply,
        })
        .await
    }

    /// Replaces the scopes of a live token; its bearer value is untouched.
    ///
    /// Deliberately separate from minting rather than an upsert: the two decisions are not the
    /// same one, and a caller that meant to widen a token must not be able to create one by
    /// misspelling an id.
    pub async fn update_capture_token_scopes(
        &self,
        id: rd_core::CaptureTokenId,
        scopes: Vec<String>,
    ) -> Result<rd_core::CaptureToken> {
        writer::request(&self.writer, |reply| {
            SessionsCommand::UpdateCaptureTokenScopes { id, scopes, reply }
        })
        .await
    }

    /// Sets or clears a live token's call limit per minute (RD-1200-04); its bearer value and
    /// scopes are untouched.
    pub async fn update_capture_token_limits(
        &self,
        id: rd_core::CaptureTokenId,
        calls_per_minute: Option<u32>,
    ) -> Result<rd_core::CaptureToken> {
        writer::request(&self.writer, |reply| {
            SessionsCommand::UpdateCaptureTokenLimits {
                id,
                calls_per_minute,
                reply,
            }
        })
        .await
    }

    /// Checks a token digest for the given scope without exposing the stored value.
    pub async fn capture_token_valid(&self, token_sha256: &str, scope: &str) -> Result<bool> {
        capture_store::token_valid_with_scope(&self.readers, token_sha256, scope).await
    }

    /// The scopes a live token holds, or `None` if the digest matches no live token.
    pub async fn capture_token_scopes(&self, token_sha256: &str) -> Result<Option<Vec<String>>> {
        capture_store::token_scopes(&self.readers, token_sha256).await
    }

    /// The id, label, scopes and calls per minute (`None`: no limit, RD-1200-04) of the live
    /// token with this digest, for the policy check, the rate and the audit log in one read.
    /// `None` when no live token has that digest.
    pub async fn capture_token_identity(
        &self,
        token_sha256: &str,
    ) -> Result<Option<(rd_core::CaptureTokenId, String, Vec<String>, Option<u32>)>> {
        capture_store::token_identity(&self.readers, token_sha256).await
    }

    /// Lists revocable connections holding any of the given scopes, without bearer values.
    pub async fn list_capture_tokens(&self, scopes: &[&str]) -> Result<Vec<rd_core::CaptureToken>> {
        capture_store::list_tokens(&self.readers, scopes).await
    }

    /// Opens a session, storing only the digest of its bearer.
    pub async fn create_session(
        &self,
        id: rd_core::SessionId,
        token_sha256: String,
        user_agent: Option<String>,
        client_ip: Option<String>,
        lifetime_hours: i64,
    ) -> Result<rd_core::Session> {
        writer::request(&self.writer, |reply| SessionsCommand::CreateSession {
            id,
            token_sha256,
            user_agent,
            client_ip,
            lifetime_hours,
            reply,
        })
        .await
    }

    /// The live session behind a bearer digest under `limits`, read without touching the
    /// writer.
    pub async fn session_for_digest(
        &self,
        token_sha256: &str,
        limits: rd_core::SessionLimits,
    ) -> Result<Option<rd_core::Session>> {
        session_store::session_for_digest(&self.readers, token_sha256, limits).await
    }

    /// Advances a session's last-used time. Reports whether it was still live.
    pub async fn touch_session(&self, token_sha256: String) -> Result<bool> {
        writer::request(&self.writer, |reply| SessionsCommand::TouchSession {
            token_sha256,
            reply,
        })
        .await
    }

    /// Every live session under `limits`, newest use first.
    pub async fn list_sessions(
        &self,
        limits: rd_core::SessionLimits,
    ) -> Result<Vec<rd_core::Session>> {
        session_store::list_sessions(&self.readers, limits).await
    }

    /// The session limits stored in the service settings (RD-130-09).
    ///
    /// Read here rather than by each caller so the service start, which only holds a
    /// database, and the settings route agree on the field names and on the defaults.
    pub async fn session_limits(&self) -> Result<rd_core::SessionLimits> {
        let idle = self.service_setting_field("session_idle_hours").await?;
        let max = self.service_setting_field("session_max_hours").await?;
        Ok(rd_core::SessionLimits::clamped(idle, max))
    }

    /// Ends one session.
    pub async fn revoke_session(&self, id: rd_core::SessionId) -> Result<bool> {
        writer::request(&self.writer, |reply| SessionsCommand::RevokeSession {
            id,
            reply,
        })
        .await
    }

    /// Ends every session but the caller's own.
    pub async fn revoke_other_sessions(&self, keep_digest: String) -> Result<u64> {
        writer::request(&self.writer, |reply| SessionsCommand::RevokeOtherSessions {
            keep_digest,
            reply,
        })
        .await
    }

    /// Ends every session, the caller's own included.
    ///
    /// What a password change needs: a change that leaves the sessions opened with the old
    /// password alive protects nothing (RD-120-22).
    pub async fn revoke_all_sessions(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| SessionsCommand::RevokeAllSessions {
            reply,
        })
        .await
    }

    /// Deletes session rows that ended under `limits` long ago.
    pub async fn purge_expired_sessions(&self, limits: rd_core::SessionLimits) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            SessionsCommand::PurgeExpiredSessions { limits, reply }
        })
        .await
    }

    /// Deletes persisted events past their retention, in bounded batches with a yield between
    /// them so the writer serves queue mutations in the gaps (DB-09).
    ///
    /// Nothing reads this table today; it is kept as the only record of what happened before
    /// the process started, and the sweep is what keeps that from becoming the largest table
    /// in the file. Run at start and by the diagnostics sweep.
    pub async fn purge_old_events(&self) -> Result<u64> {
        let mut removed = 0;
        loop {
            let batch = writer::request(&self.writer, |reply| MaintenanceCommand::PurgeOldEvents {
                reply,
            })
            .await?;
            removed += batch;
            if batch < crate::writer::EVENT_PURGE_BATCH.unsigned_abs() {
                return Ok(removed);
            }
            tokio::task::yield_now().await;
        }
    }

    /// Notes that a machine token was used, at most once a minute.
    pub async fn touch_capture_token(&self, token_sha256: String) -> Result<()> {
        writer::request(&self.writer, |reply| SessionsCommand::TouchCaptureToken {
            token_sha256,
            reply,
        })
        .await
    }

    /// Enrols a second factor. The material itself lives in the secret store.
    pub async fn create_mfa_credential(
        &self,
        id: rd_core::MfaCredentialId,
        kind: rd_core::MfaKind,
        label: String,
        material_ref: String,
    ) -> Result<rd_core::MfaCredential> {
        writer::request(&self.writer, |reply| SessionsCommand::CreateMfaCredential {
            id,
            kind,
            label,
            material_ref,
            reply,
        })
        .await
    }

    /// Every enrolled factor, newest first.
    pub async fn list_mfa_credentials(&self) -> Result<Vec<rd_core::MfaCredential>> {
        mfa_store::list_credentials(&self.readers).await
    }

    /// The vault references of every confirmed factor of one kind.
    pub async fn confirmed_mfa_material(
        &self,
        kind: rd_core::MfaKind,
    ) -> Result<Vec<(rd_core::MfaCredentialId, String)>> {
        mfa_store::confirmed_material(&self.readers, kind).await
    }

    /// The vault reference of one factor, confirmed or not.
    pub async fn mfa_material(&self, id: rd_core::MfaCredentialId) -> Result<Option<String>> {
        mfa_store::material_of(&self.readers, id).await
    }

    /// Marks a factor as proven to work.
    pub async fn confirm_mfa_credential(&self, id: rd_core::MfaCredentialId) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            SessionsCommand::ConfirmMfaCredential { id, reply }
        })
        .await
    }

    /// Accepts a TOTP code by the time step it answered, and refuses a replay of it.
    ///
    /// Takes the step rather than just noting the use: TOTP accepts a window of one step either
    /// side, so without a record of which step was spent the same six digits keep working for
    /// about ninety seconds. `false` means this step — or a later one — was already accepted,
    /// and the caller must treat the code as wrong.
    pub async fn accept_totp_step(&self, id: rd_core::MfaCredentialId, step: i64) -> Result<bool> {
        writer::request(&self.writer, |reply| SessionsCommand::AcceptTotpStep {
            id,
            step,
            reply,
        })
        .await
    }

    /// Notes that a factor answered a challenge.
    pub async fn touch_mfa_credential(&self, id: rd_core::MfaCredentialId) -> Result<()> {
        writer::request(&self.writer, |reply| SessionsCommand::TouchMfaCredential {
            id,
            reply,
        })
        .await
    }

    /// Points a factor at freshly stored material and marks it as just used.
    ///
    /// A passkey's stored state is not static: the signature counter advances and the backup
    /// flags can change, and a counter that is never written back is a counter that can never
    /// detect a cloned authenticator.
    pub async fn repoint_mfa_material(
        &self,
        id: rd_core::MfaCredentialId,
        material_ref: String,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| SessionsCommand::RepointMfaMaterial {
            id,
            material_ref,
            reply,
        })
        .await
    }

    /// Removes a factor, returning its vault reference so the secret can be deleted too.
    pub async fn delete_mfa_credential(
        &self,
        id: rd_core::MfaCredentialId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| SessionsCommand::DeleteMfaCredential {
            id,
            reply,
        })
        .await
    }

    /// Issues a fresh set of recovery codes, invalidating whatever was there.
    pub async fn replace_recovery_codes(&self, digests: Vec<String>) -> Result<()> {
        writer::request(&self.writer, |reply| {
            SessionsCommand::ReplaceRecoveryCodes { digests, reply }
        })
        .await
    }

    /// The digests of every unspent recovery code.
    pub async fn unused_recovery_digests(&self) -> Result<Vec<String>> {
        mfa_store::unused_recovery_digests(&self.readers).await
    }

    /// Spends one recovery code. Reports whether it was still unspent.
    pub async fn spend_recovery_code(&self, digest: String) -> Result<bool> {
        writer::request(&self.writer, |reply| SessionsCommand::SpendRecoveryCode {
            digest,
            reply,
        })
        .await
    }

    /// Removes every factor of one kind plus every recovery code, returning the vault
    /// references so the caller can delete the material behind them.
    pub async fn clear_mfa(&self, kind: rd_core::MfaKind) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| SessionsCommand::ClearMfa {
            kind,
            reply,
        })
        .await
    }

    /// Revokes one capture connection without deleting its audit metadata.
    pub async fn revoke_capture_token(&self, id: rd_core::CaptureTokenId) -> Result<()> {
        writer::request(&self.writer, |reply| SessionsCommand::RevokeCaptureToken {
            id,
            reply,
        })
        .await
    }
}
