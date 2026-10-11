//! The writer half of `config_store` — storage roots, categories, routing rules and hot
//! folders — and of `site_rules_store`, `site_rule_switches_store`, `site_rule_checks_store`
//! and `link_filter_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::ConfigCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_config(&mut self, command: ConfigCommand) {
        match command {
            ConfigCommand::SetCategorySeedingPolicy { id, policy, reply } => {
                let result =
                    crate::config_store::set_category_seeding(&mut self.connection, id, policy)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::SetCategoryDownloadWindow { id, window, reply } => {
                let result = crate::download_window_store::set_category_download_window(
                    &mut self.connection,
                    id,
                    window,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::CreateStorageRoot { id, input, reply } => {
                let result =
                    crate::config_store::create_storage_root(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::UpdateStorageRoot { id, input, reply } => {
                let result =
                    crate::config_store::update_storage_root(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::DeleteStorageRoot { id, reply } => {
                let result =
                    crate::config_store::delete_storage_root(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::UpsertSiteRule { input, reply } => {
                let result =
                    crate::site_rules_store::upsert_site_rule(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::SetSiteRuleSwitch {
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
            ConfigCommand::RecordSiteRuleChecks { checks, reply } => {
                let result = crate::site_rule_checks_store::record_site_rule_checks(
                    &mut self.connection,
                    checks,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::DeleteAllSiteRules { reply } => {
                let result =
                    crate::site_rules_store::delete_all_site_rules(&mut self.connection).await;
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(removed, _)| removed));
            }
            ConfigCommand::DeleteSiteRule { id, reply } => {
                let result =
                    crate::site_rules_store::delete_site_rule(&mut self.connection, &id).await;
                if let Ok((_, Some(event))) = &result {
                    let _ = self.events.send(event.clone());
                }
                send(reply, result.map(|(removed, _)| removed));
            }
            ConfigCommand::CreateCategory { input, reply } => {
                let result =
                    crate::config_store::create_category(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::UpdateCategory { id, input, reply } => {
                let result =
                    crate::config_store::update_category(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::DeleteCategory { id, reply } => {
                let result = crate::config_store::delete_category(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::UpdateCategoryPostprocess {
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
            ConfigCommand::CreateCategoryRule { input, reply } => {
                let result =
                    crate::config_store::create_category_rule(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::UpdateCategoryRule { id, input, reply } => {
                let result =
                    crate::config_store::update_category_rule(&mut self.connection, id, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::DeleteCategoryRule { id, reply } => {
                let result =
                    crate::config_store::delete_category_rule(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::CreateLinkFilterRule { input, reply } => {
                let result =
                    crate::link_filter_store::create_link_filter_rule(&mut self.connection, input)
                        .await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::UpdateLinkFilterRule { id, input, reply } => {
                let result = crate::link_filter_store::update_link_filter_rule(
                    &mut self.connection,
                    id,
                    input,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::DeleteLinkFilterRule { id, reply } => {
                let result =
                    crate::link_filter_store::delete_link_filter_rule(&mut self.connection, id)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::ReorderLinkFilterRules { ids, reply } => {
                let result =
                    crate::link_filter_store::reorder_link_filter_rules(&mut self.connection, &ids)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            ConfigCommand::CreateHotFolder { input, reply } => {
                let result =
                    crate::config_store::create_hotfolder(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::UpdateHotFolder { id, input, reply } => {
                let result =
                    crate::config_store::update_hotfolder(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            ConfigCommand::DeleteHotFolder { id, reply } => {
                let result = crate::config_store::delete_hotfolder(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
        }
    }
}
