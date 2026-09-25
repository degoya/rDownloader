//! Everything that names a remote endpoint: the writer half of `network_store`, `usenet_store`
//! and `remote_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_network(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CreateAccount { input, reply } => {
                let result =
                    crate::network_store::create_account(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateAccount { id, input, reply } => {
                let result =
                    crate::network_store::update_account(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteAccount { id, reply } => {
                let result = crate::network_store::delete_account(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::CreateProxyProfile { input, reply } => {
                let result =
                    crate::network_store::create_proxy_profile(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateProxyProfile { id, input, reply } => {
                let result =
                    crate::network_store::update_proxy_profile(&mut self.connection, id, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteProxyProfile { id, reply } => {
                let result =
                    crate::network_store::delete_proxy_profile(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::CreateUsenetServer { input, reply } => {
                let result = crate::usenet_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateUsenetServer { id, input, reply } => {
                let result = crate::usenet_store::update(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteUsenetServer { id, reply } => {
                let result = crate::usenet_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::CreateRemoteCredential { input, reply } => {
                let result = crate::remote_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateRemoteCredential { id, input, reply } => {
                let result = crate::remote_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(credential, orphaned, event)| ((credential, orphaned), event));
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteRemoteCredential { id, reply } => {
                let result = crate::remote_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::TrustSshHostKey { key, reply } => {
                let result = crate::remote_store::trust_host_key(&mut self.connection, *key).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ForgetSshHostKey {
                host,
                port,
                algorithm,
                reply,
            } => {
                let result = crate::remote_store::forget_host_key(
                    &mut self.connection,
                    &host,
                    port,
                    &algorithm,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetCandidateListing {
                id,
                listing,
                credential_id,
                reply,
            } => {
                let result = crate::remote_store::set_candidate_listing(
                    &mut self.connection,
                    id,
                    *listing,
                    credential_id,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::SetCandidateListingPlan { id, plan, reply } => {
                let result =
                    crate::remote_store::set_candidate_listing_plan(&mut self.connection, id, plan)
                        .await;
                send(reply, result);
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
