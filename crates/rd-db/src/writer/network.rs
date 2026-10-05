//! Everything that names a remote endpoint: the writer half of `network_store`, `usenet_store`
//! and `remote_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::NetworkCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_network(&mut self, command: NetworkCommand) {
        match command {
            NetworkCommand::CreateAccount { input, reply } => {
                let result =
                    crate::network_store::create_account(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::UpdateAccount { id, input, reply } => {
                let result =
                    crate::network_store::update_account(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::DeleteAccount { id, reply } => {
                let result = crate::network_store::delete_account(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::CreateProxyProfile { input, reply } => {
                let result =
                    crate::network_store::create_proxy_profile(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::UpdateProxyProfile { id, input, reply } => {
                let result =
                    crate::network_store::update_proxy_profile(&mut self.connection, id, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::DeleteProxyProfile { id, reply } => {
                let result =
                    crate::network_store::delete_proxy_profile(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::CreateUsenetServer { input, reply } => {
                let result = crate::usenet_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::UpdateUsenetServer { id, input, reply } => {
                let result = crate::usenet_store::update(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::DeleteUsenetServer { id, reply } => {
                let result = crate::usenet_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::CreateRemoteCredential { input, reply } => {
                let result = crate::remote_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::UpdateRemoteCredential { id, input, reply } => {
                let result = crate::remote_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(credential, orphaned, event)| ((credential, orphaned), event));
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::DeleteRemoteCredential { id, reply } => {
                let result = crate::remote_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::TrustSshHostKey { key, reply } => {
                let result = crate::remote_store::trust_host_key(&mut self.connection, *key).await;
                publish_unit_event(reply, result, &self.events);
            }
            NetworkCommand::ForgetSshHostKey {
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
            NetworkCommand::SetCandidateListing {
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
            NetworkCommand::SetCandidateListingPlan { id, plan, reply } => {
                let result =
                    crate::remote_store::set_candidate_listing_plan(&mut self.connection, id, plan)
                        .await;
                send(reply, result);
            }
            NetworkCommand::SetUsenetQuota { id, input, reply } => {
                let result = crate::usenet_store::set_quota(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NetworkCommand::RecordUsenetTraffic { counts, now, reply } => {
                let result =
                    crate::usenet_traffic_store::record(&mut self.connection, &counts, now)
                        .await
                        .map(|(reached, events)| {
                            for event in events {
                                let _ = self.events.send(event);
                            }
                            reached
                        });
                send(reply, result);
            }
        }
    }
}
