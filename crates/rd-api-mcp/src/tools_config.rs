//! MCP tools for settings and configuration inventory.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
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
        DefinitionParams, InstallBundledServicesParams, ListBundledServicesParams,
        RemoveBundledServicesParams, RemoveSupersededPluginVersionsParams, SetPluginEnabledParams,
        UninstallPluginParams, UpdateDefinitionParams,
    },
    params_remaining::UpdateCaptureAgentSettingsParams,
};
use crate::{ApiError, dto::SettingsResponse};

#[tool_router(router = config_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the service settings (concurrency - max_active_files 1-32, the downloads running at once, which the scheduler applies on its next pass without a restart -, speed limit, retries and the automatic retry of failed downloads - auto_retry_failed, auto_retry_interval_hours 1-24, auto_retry_max_rounds 0-100 with 0 no limit -, post-processing, fail_hopeless_jobs - whether a Usenet download that can no longer be repaired is stopped early and failed with usenet.job_hopeless, default true -, media/gallery/stream/torrent options, nzb_hand_over_linkgrabber_enabled and nzb_hand_over_downloads_enabled - whether the LinkGrabber and the Downloads view offer handing an NZB to a remote-job provider, both default true -, downloads_packages_closed_by_default (default true) and linkgrabber_packages_closed_by_default (default false) - whether a package nobody opened or closed yet starts closed in the Downloads view and in the LinkGrabber; what was opened or closed by hand is kept per browser -, account_traffic_action - what an account whose hoster reports its traffic used up does: nothing (only its failed downloads wait), pause_account (default; its other downloads wait too) or pause_queue (nothing new starts until it has traffic again) - and account_traffic_overrides, the same per account id). Pass keys to project a subset; update_settings changes them."
    )]
    pub async fn get_settings(
        &self,
        Parameters(params): Parameters<GetSettingsParams>,
    ) -> McpToolResult {
        let result = async {
            let settings = crate::settings_store::read_settings(&self.state).await?;
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
        description = "Update service settings with a partial patch of top-level keys (e.g. {\"speed_limit_bytes_per_second\": \"1000000\"}). Unknown keys are rejected; the merged result is validated and applied live. Returns the applied settings. Post-processing switches are keys of this document too, e.g. {\"direct_unpack\": true} unpacks a Usenet package's multi-volume RAR set while it still downloads (off by default; per category: update_category_postprocess), and {\"package_name_rules\": {\"spaces_to_dots\": true}} tidies the names and folders of new packages (four switches spaces_to_dots, collapse_separators, strip_bracket_tags, lowercase, all off by default; the object replaces all four, a switch left out is off; try them with preview_package_name), and {\"package_name_regex\": [{\"pattern\": \"_\", \"replacement\": \".\"}]} adds regex find → replace pairs run after those switches (at most 10, 200 characters each), and {\"unwrap_package_folder\": true} moves the content of a single folder named like the package up into the package folder after post-processing (off by default; never overwrites). Fields that decide who may sign in, what the service runs or reads, or where it sends data (executables and scripts, reconnect_enabled, reconnect_script, reconnect_ip_check_urls, passwords_file, excluded_domains_file, the trace, DLC and clamd endpoints, managed_tools_manifest_url, torrent_ip_blocklist_url) need api:admin; without it the change answers auth.scope_insufficient naming the field. A completion_script or reconnect_script not stored yet is refused with mcp.script_not_allowed unless the person allowed scripts for tools; mcp_scripts_allowed itself is changed only in the web interface (mcp.setting_outside_mcp)."
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
            let current = crate::settings_store::read_settings(&self.state).await?;
            let mut merged = serde_json::to_value(&current).map_err(anyhow::Error::new)?;
            let object = merged
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("settings did not serialize to an object"))?;
            for (key, value) in params.patch {
                if !object.contains_key(&key) {
                    return Err(ApiError::bad_request(
                        "settings.unknown_key",
                        format!("Unknown settings key: {key}"),
                    )
                    .with_param("key", key));
                }
                object.insert(key, value);
            }
            let settings: SettingsResponse = serde_json::from_value(merged)
                .map_err(|error| ApiError::bad_request("settings.invalid", error.to_string()))?;
            // Whether tools may name scripts is the person's decision about tools, so no tool
            // takes it, whatever its scopes (RD-1190-21).
            if settings.mcp_scripts_allowed != current.mcp_scripts_allowed {
                return Err(ApiError::forbidden(
                    "mcp.setting_outside_mcp",
                    "This setting is changed in the web interface, not through a tool",
                )
                .with_param("setting", "mcp_scripts_allowed"));
            }
            super::script_gate::check_settings(&self.state, &current, &settings).await?;
            // The same gate, apply and audit as the REST route, with the scopes this caller
            // holds: the tool costs `api:config`, and until RD-130-09 it skipped the check
            // that keeps a configuration token off the administrator-only fields.
            let holds_admin = super::granted_now().contains(&rd_core::Scope::Admin);
            crate::settings_handlers::save_settings(
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
        description = "Read what the desktop capture agent is set to (Settings > Clients & API > Desktop): clipboard_paused - whether it leaves the clipboard alone; Click'n'Load, the browser extension and rdownloader:// links stay on -, shortcuts - the system-wide key combination of each tray command (open, start_all, pause_all, pause_half_hour, pause_hour, clipboard_watch, send_clipboard - read the clipboard once and hand its links over, also while watching is paused -, quit), null for none -, default_shortcuts, and report: what the agent last said when it registered them (refused: commands whose combination another program holds; unavailable: wayland, no_display, no_tray or failed, when it could register none). update_capture_agent_settings changes them."
    )]
    pub async fn get_capture_agent_settings(&self) -> McpToolResult {
        respond(
            crate::capture_agent_handlers::get_capture_agent_settings(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Pause or resume the desktop capture agent's clipboard watching (clipboard_paused), or change the system-wide shortcuts of its tray commands (shortcuts: command -> combination such as \"CmdOrCtrl+Alt+V\", or null for none; commands left out keep theirs). CmdOrCtrl is Ctrl on Windows and Linux and Cmd on macOS. A combination needs two of Ctrl, Alt and Super/Cmd (Shift may come on top); one the operating system already uses is refused with capture.shortcut_reserved, one another command has with capture.shortcut_duplicate (params: command, other), an unreadable one with capture.shortcut_invalid. The agent follows within seconds, without a restart, and keeps the pause over its own restart. Linux registers shortcuts under X11 only."
    )]
    pub async fn update_capture_agent_settings(
        &self,
        Parameters(params): Parameters<UpdateCaptureAgentSettingsParams>,
    ) -> McpToolResult {
        let result = async {
            let shortcuts = match params.shortcuts {
                None => None,
                Some(changes) => {
                    let mut shortcuts =
                        crate::capture_agent_handlers::stored_capture_agent_settings(
                            &self.state.database,
                        )
                        .await?
                        .shortcuts;
                    for (name, shortcut) in changes {
                        let command = rd_core::CaptureCommand::ALL
                            .into_iter()
                            .find(|command| command.as_str() == name)
                            .ok_or_else(|| {
                                ApiError::bad_request(
                                    "capture.shortcut_command_unknown",
                                    format!("Unknown tray command: {name}"),
                                )
                                .with_param("command", &name)
                            })?;
                        shortcuts.set(command, shortcut);
                    }
                    Some(shortcuts)
                }
            };
            crate::capture_agent_handlers::update_capture_agent_settings(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(crate::capture_agent_handlers::CaptureAgentSettingsPatch {
                    clipboard_paused: params.clipboard_paused,
                    shortcuts,
                }),
            )
            .await
            .map(|Json(answer)| answer)
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
            let mut definitions = rd_api_core::automation_service::current_definitions(
                &self.state.database,
                &automations,
            )
            .await?;
            let mut rows = Vec::with_capacity(automations.len());
            for automation in automations {
                let definition = definitions.remove(&automation.id);
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
            let id = parse_id(&params.id)?;
            Ok(crate::automation_handlers::enable_automation(
                State(self.state.clone()),
                AxumPath(id),
                Json(crate::automation_handlers::EnableRequest {
                    enabled: params.enabled,
                }),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Create an automation. `definition` is the REST body of POST /api/v1/automations: name, enabled, trigger, condition, actions. Call list_automations or the /api/v1/automations/vocabulary endpoint for the trigger, field, operator and action names in force. A script action is refused with mcp.script_not_allowed unless the person allowed scripts for tools in the settings."
    )]
    pub async fn create_automation(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            super::script_gate::check_automation(&self.state, &params.definition, &[]).await?;
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
        description = "Replace one automation, creating a new version of it. `definition` is the same body as create; it is a replacement, so send the whole definition. A script action the automation does not carry yet is refused with mcp.script_not_allowed unless the person allowed scripts for tools in the settings."
    )]
    pub async fn update_automation(
        &self,
        Parameters(params): Parameters<UpdateDefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let stored = super::script_gate::stored_automation_scripts(&self.state, id).await?;
            super::script_gate::check_automation(&self.state, &params.definition, &stored).await?;
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

    #[tool(
        description = "Uninstall every superseded plugin version at once: of every plugin, or of the one named by id. A version is superseded when another version of the same plugin is the one that runs; that one is never removed. Kept and listed under kept, each with its reason code: a version an unfinished download is still bound to (plugin.version_in_use), the version the next start loads, e.g. right after an update (plugin.version_next_start), and the version under test (plugin.version_under_test). Answers removed and kept; plugin.superseded_none when there was nothing to remove."
    )]
    pub async fn remove_superseded_plugin_versions(
        &self,
        Parameters(params): Parameters<RemoveSupersededPluginVersionsParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_handlers::remove_superseded(
                &self.state,
                &crate::audit::AuditContext::current(),
                params.id.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        description = "List the services the release ships as plugins (hosters, multihosters, remote jobs, cloud drives, link formats, metadata, notifications, post-processing), each with its plugins and whether it is installed, partly installed or available. Only the chosen ones are installed; the rest are offered here."
    )]
    pub async fn list_bundled_services(
        &self,
        Parameters(params): Parameters<ListBundledServicesParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_bundled::list_bundled_services(
                State(self.state.clone()),
                Query(crate::plugin_bundled::BundledCatalogueQuery {
                    locale: params.locale,
                }),
            )
            .await
            .map(|catalogue| catalogue.0),
        )
    }

    #[tool(
        description = "Install bundled services by key, as list_bundled_services reports them: every plugin of each service that is not installed yet. Their provider rows are live at once, so accounts can be added straight away; a first install of a hoster or sign-in plugin runs at once too, and anything else from the next service start (restart_required says so). Answers what was installed and what failed."
    )]
    pub async fn install_bundled_services(
        &self,
        Parameters(params): Parameters<InstallBundledServicesParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_bundled::install_bundled_services(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(crate::plugin_bundled::BundledInstallRequest {
                    services: params.services,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Remove bundled services by key, as list_bundled_services reports them: every installed version of every plugin of each service. Their provider rows go at once; the plugins stop at the next service start. A service a download that has not finished is still bound to stays installed and is listed under failed (plugin.version_in_use); the others are removed. A later start does not install a removed service again."
    )]
    pub async fn remove_bundled_services(
        &self,
        Parameters(params): Parameters<RemoveBundledServicesParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_bundled::remove_bundled_services(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(crate::plugin_bundled::BundledRemoveRequest {
                    services: params.services,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }
}
