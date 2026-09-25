//! The writer half of `config_store` — storage roots, categories, routing rules and hot
//! folders — and of `site_rules_store`, `site_rule_switches_store` and
//! `site_rule_checks_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_config(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::SetCategorySeedingPolicy { id, policy, reply } => {
                let result =
                    crate::config_store::set_category_seeding(&mut self.connection, id, policy)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::CreateStorageRoot { id, input, reply } => {
                let result =
                    crate::config_store::create_storage_root(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateStorageRoot { id, input, reply } => {
                let result =
                    crate::config_store::update_storage_root(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteStorageRoot { id, reply } => {
                let result =
                    crate::config_store::delete_storage_root(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::UpsertSiteRule { input, reply } => {
                let result =
                    crate::site_rules_store::upsert_site_rule(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetSiteRuleSwitch {
                scope,
                key,
                enabled,
                reply,
            } => {
                let result = crate::site_rule_switches_store::set_site_rule_switch(
                    &mut self.connection,
                    &scope,
                    &key,
                    enabled,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::RecordSiteRuleChecks { checks, reply } => {
                let result = crate::site_rule_checks_store::record_site_rule_checks(
                    &mut self.connection,
                    checks,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::DeleteSiteRule { id, reply } => {
                let result =
                    crate::site_rules_store::delete_site_rule(&mut self.connection, &id).await;
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(removed, _)| removed));
            }
            WriterCommand::CreateCategory { input, reply } => {
                let result =
                    crate::config_store::create_category(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateCategory { id, input, reply } => {
                let result =
                    crate::config_store::update_category(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteCategory { id, reply } => {
                let result = crate::config_store::delete_category(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::UpdateCategoryPostprocess {
                id,
                postprocess,
                reply,
            } => {
                let result = crate::config_store::update_category_postprocess(
                    &mut self.connection,
                    id,
                    postprocess,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::CreateCategoryRule { input, reply } => {
                let result =
                    crate::config_store::create_category_rule(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateCategoryRule { id, input, reply } => {
                let result =
                    crate::config_store::update_category_rule(&mut self.connection, id, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteCategoryRule { id, reply } => {
                let result =
                    crate::config_store::delete_category_rule(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::CreateHotFolder { input, reply } => {
                let result =
                    crate::config_store::create_hotfolder(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateHotFolder { id, input, reply } => {
                let result =
                    crate::config_store::update_hotfolder(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteHotFolder { id, reply } => {
                let result = crate::config_store::delete_hotfolder(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
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
