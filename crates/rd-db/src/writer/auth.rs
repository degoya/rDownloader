//! Authentication against a provider: the writer half of `auth_flow_store` and `auth_profile_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::AuthCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_auth(&mut self, command: AuthCommand) {
        match command {
            AuthCommand::UpsertAuthFlow { input, reply } => {
                let result = crate::auth_flow_store::upsert(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::SetAuthFlowRenewal {
                account_id,
                token_expires_at,
                refresh_ref,
                access_ref,
                reply,
            } => {
                let result = crate::auth_flow_store::set_renewal(
                    &mut self.connection,
                    account_id,
                    token_expires_at,
                    refresh_ref.as_deref(),
                    access_ref.as_deref(),
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::SetAuthFlowSession {
                account_id,
                access_ref,
                key_ref,
                reply,
            } => {
                let result = crate::auth_flow_store::set_session(
                    &mut self.connection,
                    account_id,
                    &access_ref,
                    key_ref.as_deref(),
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            AuthCommand::SetAuthFlowPart {
                account_id,
                name,
                secret_ref,
                reply,
            } => {
                let result = crate::auth_flow_store::set_part(
                    &mut self.connection,
                    account_id,
                    &name,
                    &secret_ref,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::DeferAuthFlowRenewal {
                account_id,
                next_poll_at,
                reply,
            } => {
                let result = crate::auth_flow_store::defer_renewal(
                    &mut self.connection,
                    account_id,
                    next_poll_at,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            AuthCommand::DeleteAuthFlow { account_id, reply } => {
                let result = crate::auth_flow_store::delete(&mut self.connection, account_id).await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::TakeAuthFlowCallback {
                callback_state,
                reply,
            } => {
                let result =
                    crate::auth_flow_store::take_callback(&mut self.connection, &callback_state)
                        .await;
                send(reply, result);
            }
            AuthCommand::SetDownloadAuthProfile {
                id,
                selection,
                reply,
            } => {
                let result = crate::auth_profile_store::set_download_selection(
                    &mut self.connection,
                    id,
                    selection,
                )
                .await;
                send(reply, result);
            }
            AuthCommand::CreateAuthProfile { input, reply } => {
                let result = crate::auth_profile_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::UpdateAuthProfile { id, input, reply } => {
                let result = crate::auth_profile_store::update(&mut self.connection, id, input)
                    .await
                    .map(|(profile, orphaned, event)| ((profile, orphaned), event));
                publish_config(reply, result, &self.events);
            }
            AuthCommand::SetAuthProfileEnabled { id, enabled, reply } => {
                let result =
                    crate::auth_profile_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            AuthCommand::DeleteAuthProfile { id, reply } => {
                let result = crate::auth_profile_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
        }
    }
}
