//! MCP tools for settings and configuration inventory.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use axum::extract::State;
use rd_api_core::extract::{Json, Path as AxumPath, Query};

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
use crate::ApiError;

#[tool_router(router = config_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the service settings; pass keys for a subset, update_settings changes them. Among them: max_active_files (1-32 downloads at once, applied on the scheduler's next pass); sequential_packages (default false; true: a download starts only while no package ahead of its own in the list, by priority then position, has a file queued or waiting for its retry; running files hold nothing, a first package waiting for a hoster's limit or used-up traffic holds the whole list); speed limit; max_retries 0-100 (five within seconds, then retry_middle_attempts 0-10 waits of 1, 2, 4 minutes and on, then retry_long_wait_minutes 1-1440 between the rest); auto_retry_failed, auto_retry_interval_hours 1-24, auto_retry_max_rounds 0-100 (0 no limit); clear_failed_delete_files (default true: clear_finished_packages with scope failed deletes every file of those packages from disk, finished ones included; false: only what did not finish); package_reset_delete_extracted (default true: reset_package also deletes what the unpack created; false: only the downloads); post-processing; fail_hopeless_jobs (default true: a Usenet download beyond repair is stopped early and failed with usenet.job_hopeless); media/gallery/stream/torrent options; media_sleep_requests_seconds and media_sleep_interval_seconds (0-600, 0 none: yt-dlp's pauses between a media download's requests and before it starts; a link's criteria.pauses override them); nzb_hand_over_linkgrabber_enabled, nzb_hand_over_downloads_enabled, nzb_hand_over_indexers_enabled (default true: the LinkGrabber, the Downloads view and a hit in the indexer review box, hand_over_subscription_item, offer handing an NZB to a remote-job provider); linkgrabber_hand_over_enabled (default true: a LinkGrabber package's menu offers the same for its links); linkgrabber_hand_over_remove_links (default true: links hand_over_collector_package sent leave the LinkGrabber, a package left empty too; false: they stay marked handed_over and a second hand-over sends nothing again); duplicates_include_history (default false; true: the LinkGrabber's duplicate check compares with the download history besides the queue); downloads_packages_closed_by_default (default true) and linkgrabber_packages_closed_by_default (default false: whether a package nobody opened or closed yet starts closed there; kept per browser once opened or closed by hand); account_traffic_action (an account whose hoster reports its traffic used up: nothing - only its failed downloads wait -, pause_account - default, its other downloads wait too -, pause_queue - nothing new starts until it has traffic) and account_traffic_overrides, the same per account id."
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
        description = "Update service settings with a partial patch of top-level keys (e.g. {\"speed_limit_bytes_per_second\": \"1000000\"}). Keys left out keep their value, an object or a list is replaced whole, null empties a key that can be empty and is refused for any other (settings.invalid). Unknown keys are rejected; the merged result is validated and applied live. Returns the applied settings. Keys among them, off by default unless said: direct_unpack (unpacks a Usenet package's multi-volume RAR set while it downloads; per category: update_category_postprocess); package_name_rules {spaces_to_dots, collapse_separators, strip_bracket_tags, lowercase} (tidies names and folders of new packages; replaces all four, one left out is off) and package_name_regex [{pattern, replacement}] (run after those; at most 10, 200 characters each) - try both with preview_package_name; unwrap_package_folder (moves the content of a single folder named like the package up into the package folder after post-processing, never overwrites); refuse_unknown_archive_tool_version (no unpacking with an unrar or 7-Zip whose version, and so its security minimum, cannot be read: the set fails with extract.tool_version_unknown; off, it unpacks with a warning); aria2_rpc_enabled (the aria2 JSON-RPC endpoint /jsonrpc for AriaNg and the like; its RPC secret is an API token holding api:intake, api:queue and api:read); log_level (error, warn, info - default -, debug; from the next line; set_log_level does the same, start_log_debug puts one area on debug for a while). Host names a browser may call the service by: addresses, localhost, the external_url's host and allowed_hosts always pass; accept_local_network_names (default true) also passes names only the local network knows (one label like nas, .local, .home.arpa, .internal), false refuses them with request.host_not_allowed until allowed_hosts names them. allowed_hosts and accept_local_network_names, like the rest of the proxy contract, and every field that decides who may sign in, what the service runs or reads, or where it sends data need api:admin; without it the change answers auth.scope_insufficient naming the field: executables and scripts, reconnect_enabled, reconnect_script, reconnect_ip_check_urls, passwords_file, excluded_domains_file, the trace, DLC and clamd endpoints, managed_tools_manifest_url, torrent_ip_blocklist_url, update_auto_install with update_auto_install_window {start_minute, end_minute} (minutes after midnight in bandwidth_timezone; the service installs an offered update by itself), restart_when_needed (it restarts by itself when a plugin installed or updated runs only from the next start, once nothing has run for five minutes, inside that window), update_backup_retention_days (how long the newest backup before an update stays once proven, 0 for good), subscription_item_retention_days (how long a skipped or dismissed subscription item keeps more than its key, default 30, 0 for good). A completion_script or reconnect_script not stored yet is refused with mcp.script_not_allowed unless the person allowed scripts for tools; mcp_scripts_allowed itself is changed only in the web interface (mcp.setting_outside_mcp). A torrent setting the engine cannot take answers torrent.session_rebuild_failed with the reason, a fixed listen port another program holds torrent.port_in_use with the port: the settings are saved, the engine keeps its previous ones."
    )]
    pub async fn update_settings(
        &self,
        Parameters(params): Parameters<UpdateSettingsParams>,
    ) -> McpToolResult {
        let result = async {
            let edit = crate::settings_handlers::SettingsEdit::begin(&self.state).await?;
            let settings = edit.patched(params.patch)?;
            // Whether tools may name scripts is the person's decision about tools, so no tool
            // takes it, whatever its scopes (RD-1190-21).
            if settings.mcp_scripts_allowed != edit.current().mcp_scripts_allowed {
                return Err(ApiError::forbidden(
                    "mcp.setting_outside_mcp",
                    "This setting is changed in the web interface, not through a tool",
                )
                .with_param("setting", "mcp_scripts_allowed"));
            }
            super::script_gate::check_settings(&self.state, edit.current(), &settings).await?;
            // The same merge, gate, apply and audit as `PATCH /api/v1/settings`, with the scopes
            // this caller holds: the tool costs `api:config`, and until RD-130-09 it skipped the
            // check that keeps a configuration token off the administrator-only fields.
            let holds_admin = super::granted_now().contains(&rd_core::Scope::Admin);
            edit.save_settings(
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
        description = "Read what the desktop capture agent is set to (Settings > Clients & API > Desktop); update_capture_agent_settings changes it. clipboard_paused: it leaves the clipboard alone (Click'n'Load, the browser extension and rdownloader:// links stay on). shortcuts: the system-wide key combination of each tray command, null for none; every tray entry but the title and status lines can take one: open, start_all, pause_all, pause_half_hour, pause_hour, clipboard_watch, send_clipboard (hands the clipboard's links over once, also while watching is paused), game_mode (\"Pause while gaming\" on or off), install_server_update (the service's offered update with the agent's capture:server_update right, else the update page), auto_install (the agent's switch to install its own updates, where installed without the service), quit, add_all_from_linkgrabber and add_all_from_linkgrabber_paused (everything in the LinkGrabber to the queue, started or paused, with capture:queue), install_update (the agent's own offered update, where it installs itself), restart_server (when a restart is pending; shown only with capture:server_update), about (Settings > About of the connected app in the browser), link (the agent's page to pair it with a server), autostart (its start at login, where installed without the service); game_mode, install_server_update, auto_install, quit and the seven after quit have none by default. default_shortcuts. report: what the agent last said when registering them (refused: commands whose combination another program holds; unavailable: wayland, no_display, no_tray or failed, when it registered none). game_mode: while enabled (also from the tray) and a full-screen program is in front (full_screen, Windows only) or one of processes runs, the agent pauses the queue and renews the pause (action pause) or switches on bandwidth profile profile_id (action profile), and lifts only what it set itself once that ends."
    )]
    pub async fn get_capture_agent_settings(&self) -> McpToolResult {
        respond(
            crate::capture_agent_handlers::get_capture_agent_settings(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Pause or resume the desktop capture agent's clipboard watching (clipboard_paused), or change the system-wide shortcuts of its tray commands (shortcuts: command -> combination such as \"CmdOrCtrl+Alt+V\", or null for none; commands left out keep theirs). The commands are those get_capture_agent_settings lists; an unknown one is refused with capture.shortcut_command_unknown. A command whose tray entry is unavailable does nothing when pressed and says why: without capture:queue the LinkGrabber entries are refused, without an offered update install_update and install_server_update install nothing, without capture:server_update or a pending restart restart_server restarts nothing, beside the service autostart switches nothing. CmdOrCtrl is Ctrl on Windows and Linux and Cmd on macOS. A combination needs two of Ctrl, Alt and Super/Cmd (Shift may come on top); one the operating system already uses is refused with capture.shortcut_reserved, one another command has with capture.shortcut_duplicate (params: command, other), an unreadable one with capture.shortcut_invalid. game_mode replaces the game mode as a whole (enabled - on when left out; off keeps the rest and lifts the agent's running pause -, full_screen, processes, action pause or profile, profile_id); a process name with a path is refused with capture.game_mode_process_invalid, more than 64 names with capture.game_mode_too_many_processes, action profile without a profile with capture.game_mode_profile_missing, an unknown profile with bandwidth.profile_not_found. The agent follows within seconds, without a restart, and keeps the pause over its own restart. Linux registers shortcuts under X11 only and has no game mode."
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
                    game_mode: params
                        .game_mode
                        .map(|game_mode| crate::params_handling::body(serde_json::json!(game_mode)))
                        .transpose()?,
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
        description = "Create an automation. `definition` is the REST body of POST /api/v1/automations: name, enabled, trigger, schedule, condition, actions. The trigger `schedule` runs at a time and needs `schedule`: {\"kind\":\"cron\",\"expression\":\"0 6 * * *\"} or {\"kind\":\"interval\",\"minutes\":60}, read in the service's time zone; a missed time is not caught up, and it takes no package action. set_category {category_id} moves the package and its data into that category's folder, once any post-processing of it has ended. Besides webhook, script, set_category, pause_package and resume_package the actions are set_priority {priority: low|normal|high}, pause_queue (pauses the whole queue until start_queue), start_queue, extract_package, notify {target_id, message} and add_links {links, destination: link_grabber|downloads}. `queue_paused`, `queue_resumed` and `queue_finished` run once when the whole queue pauses, its pause ends or it ran empty, name no package and take no package action; for one cause of a pause add the condition {\"type\":\"predicate\",\"predicate\":{\"field\":\"source\",\"operator\":\"equals\",\"value\":\"schedule\"}} (or manual, automation, stop_mark, game_mode). `package_completed` and `package_failed` run once per transition into that state, not again for a reorder, a rename or the same state written again. `storage_threshold` runs when a storage root runs out of space; for when it has room again, the same condition shape with field `change`, value `released` (a storage condition that does not name `change` hears the block only); for one storage root, field `storage_target` with the `target` get_storage_capacity lists: the root's id, or `fallback` for the download directory. Call list_automations or the /api/v1/automations/vocabulary endpoint for the trigger, field, operator and action names in force. A script action is refused with mcp.script_not_allowed unless the person allowed scripts for tools in the settings."
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
        description = "List the services the release ships as plugins (hosters, multihosters, remote jobs, cloud drives, link formats, metadata, notifications, post-processing), each with its plugins and whether it is installed, partly installed or available. Per plugin: origin (where an install takes it from now: repository when the plugin repository offers it and answers, bundled otherwise), installed_origin (repository, bundled or upload), and the update waiting for the installed version. repository_services lists what only a repository offers; those install through the plugin preview, not install_bundled_services. Only the chosen services are installed; the rest are offered here."
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
        description = "Install bundled services by key, as list_bundled_services reports them: every plugin of each service that is not installed yet, from the plugin repository when it can be fetched there and from the bundle otherwise (each installed plugin's origin says which). Their provider rows are live at once, so accounts can be added straight away; a first install of a hoster or sign-in plugin runs at once too, and anything else from the next service start (restart_required says so). Answers what was installed and what failed."
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
