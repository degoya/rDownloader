//! Authentication against a provider: the writer half of `auth_flow_store` and `auth_profile_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_auth(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::UpsertAuthFlow { input, reply } => {
                let result = crate::auth_flow_store::upsert(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetAuthFlowRenewal {
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
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetAuthFlowSession {
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
            WriterCommand::DeferAuthFlowRenewal {
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
            WriterCommand::DeleteAuthFlow { account_id, reply } => {
                let result = crate::auth_flow_store::delete(&mut self.connection, account_id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetDownloadAuthProfile {
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
            WriterCommand::CreateAuthProfile { input, reply } => {
                let result = crate::auth_profile_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateAuthProfile { id, input, reply } => {
                let result = crate::auth_profile_store::update(&mut self.connection, id, input)
                    .await
                    .map(|(profile, orphaned, event)| ((profile, orphaned), event));
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetAuthProfileEnabled { id, enabled, reply } => {
                let result =
                    crate::auth_profile_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteAuthProfile { id, reply } => {
                let result = crate::auth_profile_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            // `Writer::run` routes every variant to exactly one handler, and its match is
            // exhaustive over `WriterCommand`, so nothing reaches this arm. It drops the
            // command instead of panicking: a mis-routed command must not take down the one
            // task every mutation in the process runs on, and the caller already treats a
            // dropped reply as a failed request.
            _ => {}
        }
    }
}
