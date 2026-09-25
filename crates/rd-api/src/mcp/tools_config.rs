//! MCP tools for settings and configuration inventory.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, from_definition, json_result, parse_id, respond},
    params::{
        ConfigSection, GetSettingsParams, ListAutomationsParams, ListConfigurationParams,
        ToggleAutomationParams, UpdateSettingsParams,
    },
    params_config::IdParams,
    params_delivery::{
        DefinitionParams, SetPluginEnabledParams, UninstallPluginParams, UpdateDefinitionParams,
    },
};
use crate::{ApiError, dto::SettingsResponse};

#[tool_router(router = config_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the service settings (concurrency, speed limit, post-processing, media/gallery/stream/torrent options). Pass keys to project a subset."
    )]
    pub async fn get_settings(
        &self,
        Parameters(params): Parameters<GetSettingsParams>,
    ) -> McpToolResult {
        let result = async {
            let settings = crate::handlers::read_settings(&self.state).await?;
            let mut value = serde_json::to_value(&settings).map_err(anyhow::Error::new)?;
            if let Some(keys) = params.keys.filter(|keys| !keys.is_empty())
                && let Some(map) = value.as_object_mut()
            {
                map.retain(|key, _| keys.iter().any(|wanted| wanted == key));
            }
            Ok(value)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Update service settings with a partial patch of top-level keys (e.g. {\"speed_limit_bytes_per_second\": \"1000000\"}). Unknown keys are rejected; the merged result is validated and applied live. Returns the applied settings."
    )]
    pub async fn update_settings(
        &self,
        Parameters(params): Parameters<UpdateSettingsParams>,
    ) -> McpToolResult {
        if params.patch.is_empty() {
            return Ok(api_error(ApiError::bad_request(
                "settings.patch_empty",
                "The patch must contain at least one settings key",
            )));
        }
        let result = async {
            let current = crate::handlers::read_settings(&self.state).await?;
            let mut merged = serde_json::to_value(&current).map_err(anyhow::Error::new)?;
            let object = merged
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("settings did not serialize to an object"))?;
            for (key, value) in params.patch {
                if !object.contains_key(&key) {
                    return Err(ApiError::bad_request(
                        "settings.unknown_key",
                        format!("Unknown settings key: {key}"),
                    ));
                }
                object.insert(key, value);
            }
            let settings: SettingsResponse = serde_json::from_value(merged)
                .map_err(|error| ApiError::bad_request("settings.invalid", error.to_string()))?;
            // The same gate, apply and audit as the REST route, with the scopes this caller
            // holds: the tool costs `api:config`, and until RD-130-09 it skipped the check
            // that keeps a configuration token off the administrator-only fields.
            let holds_admin = super::granted_now().contains(&rd_core::Scope::Admin);
            crate::handlers::save_settings(
                &self.state,
                &crate::audit::AuditContext::current(),
                holds_admin,
                settings,
            )
            .await
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List configuration entries: categories, storage_roots, accounts (credentials redacted), proxy_profiles, providers (supported hosters) or plugins. The plugins section answers with the whole inventory -- `installed` and `incompatible` -- each installed row carrying whether it is the version in force (`active`) and how often it has run."
    )]
    pub async fn list_configuration(
        &self,
        Parameters(params): Parameters<ListConfigurationParams>,
    ) -> McpToolResult {
        let result: Result<serde_json::Value, ApiError> = async {
            let value = match params.section {
                ConfigSection::Categories => {
                    serde_json::to_value(self.state.database.list_categories().await?)
                }
                ConfigSection::StorageRoots => {
                    // Carries the persistence verdict too: an assistant asked why downloads
                    // vanished should be able to see the same thing the routing view shows.
                    let probe = rd_files::PersistenceProbe::detect();
                    let roots: Vec<_> = self
                        .state
                        .database
                        .list_storage_roots()
                        .await?
                        .into_iter()
                        .map(|root| crate::dto::StorageRootResponse {
                            persistence: probe.classify(std::path::Path::new(&root.path)).into(),
                            root,
                        })
                        .collect();
                    serde_json::to_value(roots)
                }
                ConfigSection::Accounts => {
                    serde_json::to_value(self.state.database.list_accounts().await?)
                }
                ConfigSection::ProxyProfiles => {
                    serde_json::to_value(self.state.database.list_proxy_profiles().await?)
                }
                ConfigSection::Providers => serde_json::to_value(
                    rd_provider_registry::all()
                        .iter()
                        .map(crate::dto::ProviderResponse::from)
                        .collect::<Vec<_>>(),
                ),
                // Delegated rather than rebuilt from the manifests, which is what this
                // section used to do (RD-120-29). `InstalledPluginResponse::from` sees a
                // manifest and nothing else, so it reported `active: false` for every plugin
                // -- including the version that actually wins at load time -- and RD-120-28's
                // `execution_count`, read from the execution store, would have come back `0`
                // for a plugin that has run a thousand times. A wrong number is worse than a
                // missing one here: a model reading `0` concludes the plugin never ran. The
                // route computes both; this asks the route. The answer is therefore the whole
                // inventory, incompatible plugins included, where the tool used to drop them.
                ConfigSection::Plugins => serde_json::to_value(
                    crate::plugin_handlers::list_plugins(State(self.state.clone()))
                        .await?
                        .0,
                ),
            };
            value.map_err(|error| anyhow::Error::new(error).into())
        }
        .await;
        match result {
            Ok(value) => json_result(&value),
            Err(error) => Ok(api_error(error)),
        }
    }

    #[tool(
        description = "List the configured automations with the definition currently in force. Set with_runs to include the recent run history of each one."
    )]
    pub async fn list_automations(
        &self,
        Parameters(params): Parameters<ListAutomationsParams>,
    ) -> McpToolResult {
        let result = async {
            let automations = self.state.database.list_automations().await?;
            let mut rows = Vec::with_capacity(automations.len());
            for automation in automations {
                let definition = self
                    .state
                    .database
                    .automation_versions(automation.id)
                    .await?
                    .into_iter()
                    .find(|version| version.version == automation.version);
                let runs = if params.with_runs.unwrap_or(false) {
                    Some(
                        self.state
                            .database
                            .automation_runs(Some(automation.id), 20)
                            .await?,
                    )
                } else {
                    None
                };
                rows.push(serde_json::json!({
                    "automation": automation,
                    "definition": definition,
                    "runs": runs,
                }));
            }
            Ok::<_, ApiError>(serde_json::Value::Array(rows))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Enable or disable one automation. Disabling stops new runs; runs already in flight finish."
    )]
    pub async fn toggle_automation(
        &self,
        Parameters(params): Parameters<ToggleAutomationParams>,
    ) -> McpToolResult {
        let result = async {
            let id = super::error::parse_id(&params.id)?;
            let automation = self
                .state
                .database
                .set_automation_enabled(id, params.enabled)
                .await
                .map_err(|error| {
                    crate::error_codes::store_not_found(
                        &error,
                        "automation.not_found",
                        "Automation not found",
                    )
                })?;
            serde_json::to_value(&automation).map_err(|error| anyhow::Error::new(error).into())
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Create an automation. `definition` is the REST body of POST /api/v1/automations: name, enabled, trigger, condition, actions. Call list_automations or the /api/v1/automations/vocabulary endpoint for the trigger, field, operator and action names in force."
    )]
    pub async fn create_automation(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = from_definition(params.definition)?;
            Ok(crate::automation_handlers::create_automation(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .1
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Replace one automation, creating a new version of it. `definition` is the same body as create; it is a replacement, so send the whole definition."
    )]
    pub async fn update_automation(
        &self,
        Parameters(params): Parameters<UpdateDefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = from_definition(params.definition)?;
            Ok(crate::automation_handlers::update_automation(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one automation with its versions and run history. Destructive; use toggle_automation to only stop it running."
    )]
    pub async fn delete_automation(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            Ok(crate::automation_handlers::delete_automation(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Switch one installed plugin off or back on. It stays installed and listed; the change takes full effect on the next service start."
    )]
    pub async fn set_plugin_enabled(
        &self,
        Parameters(params): Parameters<SetPluginEnabledParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_handlers::set_plugin_enabled(
                State(self.state.clone()),
                AxumPath(params.id),
                Json(crate::plugin_handlers::PluginEnabledRequest {
                    enabled: params.enabled,
                }),
            )
            .await
            .map(|message| message.0),
        )
    }

    #[tool(
        description = "Uninstall one installed plugin version. Destructive: the provider it contributed disappears at once, and accounts using it can no longer be served. Refused with plugin.version_in_use while an unfinished download is still bound to that exact version by its resolver pin or its transfer checkpoint."
    )]
    pub async fn uninstall_plugin_version(
        &self,
        Parameters(params): Parameters<UninstallPluginParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_handlers::remove_plugin_version(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                AxumPath((params.id, params.version)),
            )
            .await
            .map(|message| message.0),
        )
    }
}
