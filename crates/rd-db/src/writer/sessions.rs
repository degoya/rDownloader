//! Who may talk to this service: the writer half of `session_store`, `capture_store` and
//! `mfa_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::SessionsCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_sessions(&mut self, command: SessionsCommand) {
        match command {
            SessionsCommand::CreateSession {
                id,
                token_sha256,
                user_agent,
                client_ip,
                lifetime_hours,
                reply,
            } => {
                send(
                    reply,
                    crate::session_store::create_session(
                        &mut self.connection,
                        id,
                        token_sha256,
                        user_agent,
                        client_ip,
                        lifetime_hours,
                    )
                    .await,
                );
            }
            SessionsCommand::TouchSession {
                token_sha256,
                reply,
            } => {
                send(
                    reply,
                    crate::session_store::touch_session(&mut self.connection, &token_sha256).await,
                );
            }
            SessionsCommand::RevokeSession { id, reply } => {
                send(
                    reply,
                    crate::session_store::revoke_session(&mut self.connection, id).await,
                );
            }
            SessionsCommand::RevokeOtherSessions { keep_digest, reply } => {
                send(
                    reply,
                    crate::session_store::revoke_other_sessions(&mut self.connection, &keep_digest)
                        .await,
                );
            }
            SessionsCommand::RevokeAllSessions { reply } => {
                send(
                    reply,
                    crate::session_store::revoke_all(&mut self.connection).await,
                );
            }
            SessionsCommand::PurgeExpiredSessions { limits, reply } => {
                send(
                    reply,
                    crate::session_store::purge_expired(&mut self.connection, limits).await,
                );
            }
            SessionsCommand::TouchCaptureToken {
                token_sha256,
                reply,
            } => {
                send(
                    reply,
                    crate::session_store::touch_capture_token(&mut self.connection, &token_sha256)
                        .await,
                );
            }
            SessionsCommand::CreateCaptureToken {
                id,
                label,
                token_sha256,
                scopes,
                expires_at,
                calls_per_minute,
                reply,
            } => {
                let result = crate::capture_store::create_token(
                    &mut self.connection,
                    id,
                    label,
                    token_sha256,
                    scopes,
                    expires_at,
                    calls_per_minute,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            SessionsCommand::UpdateCaptureTokenLimits {
                id,
                calls_per_minute,
                reply,
            } => {
                let result = crate::capture_store::update_token_limits(
                    &mut self.connection,
                    id,
                    calls_per_minute,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            SessionsCommand::UpdateCaptureTokenScopes { id, scopes, reply } => {
                let result =
                    crate::capture_store::update_token_scopes(&mut self.connection, id, scopes)
                        .await;
                publish_config(reply, result, &self.events);
            }
            SessionsCommand::RevokeCaptureToken { id, reply } => {
                let result = crate::capture_store::revoke_token(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            SessionsCommand::CreateMfaCredential {
                id,
                kind,
                label,
                material_ref,
                reply,
            } => {
                send(
                    reply,
                    crate::mfa_store::create_credential(
                        &mut self.connection,
                        id,
                        kind,
                        label,
                        material_ref,
                    )
                    .await,
                );
            }
            SessionsCommand::ConfirmMfaCredential { id, reply } => {
                send(
                    reply,
                    crate::mfa_store::confirm_credential(&mut self.connection, id).await,
                );
            }
            SessionsCommand::RepointMfaMaterial {
                id,
                material_ref,
                reply,
            } => {
                send(
                    reply,
                    crate::mfa_store::repoint_material(&mut self.connection, id, material_ref)
                        .await,
                );
            }
            SessionsCommand::TouchMfaCredential { id, reply } => {
                send(
                    reply,
                    crate::mfa_store::touch_credential(&mut self.connection, id).await,
                );
            }
            SessionsCommand::AcceptTotpStep { id, step, reply } => {
                send(
                    reply,
                    crate::mfa_store::accept_totp_step(&mut self.connection, id, step).await,
                );
            }
            SessionsCommand::DeleteMfaCredential { id, reply } => {
                send(
                    reply,
                    crate::mfa_store::delete_credential(&mut self.connection, id).await,
                );
            }
            SessionsCommand::ReplaceRecoveryCodes { digests, reply } => {
                send(
                    reply,
                    crate::mfa_store::replace_recovery_codes(&mut self.connection, digests).await,
                );
            }
            SessionsCommand::SpendRecoveryCode { digest, reply } => {
                send(
                    reply,
                    crate::mfa_store::spend_recovery_code(&mut self.connection, &digest).await,
                );
            }
            SessionsCommand::ClearMfa { kind, reply } => {
                send(
                    reply,
                    crate::mfa_store::clear_all(&mut self.connection, kind).await,
                );
            }
        }
    }
}
