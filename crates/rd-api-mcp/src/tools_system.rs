//! MCP tools for post-processing, the managed external tools and storage capacity
//! (RD-120-32).
//!
//! The post-processing lists were out because they are dropdowns, and the reason given was that
//! a status token must not enumerate the installation. That reason is about the *price*, and
//! the price is `scope_policy`'s: these tools cost `api:queue` exactly as their routes do, so a
//! status token still cannot read them. Managed tools and storage capacity were out as
//! administration and as "revisit if the code on the row proves too thin"; both are now one
//! call to the route that answers the screen.
//!
//! `get_update_status` and `check_for_updates` are the update card of Settings > System
//! (RD-180-01): what runs, what is offered and what to run for it. `get_restart_status` and
//! `restart_service` its restart notice (RD-1240-32): what waits for the next start, and the
//! restart that applies it. Unlike installing an update, a restart replaces nothing and brings
//! the same version back; the session that asked ends with the process and the agent connects
//! again, as after any restart.
//!
//! `get_about` reads the About page's head (RD-130-12). Not its licence list: a thousand
//! entries answer no question an agent is asked, and the route stays one call away for a person.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_handling::{
        IdBodyParams, MalwareScannerTestParams, ManageAction, ManageToolParams, ManagedToolsParams,
        ManagedToolsView, PackageNamePreviewParams, PostprocessOptions, PostprocessOptionsParams,
        SortPreviewParams, StorageTargetParams, body,
    },
    script_gate,
};
use crate::{
    ApiError, postprocess_handlers as postprocess, restart_handlers as restarts,
    tools_handlers as tools, update_handlers as updates,
};

/// Whether `restart_service` goes ahead while downloads run.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct RestartParams {
    /// Restart although downloads are running: the stop saves them and they continue after the
    /// restart. Without it running downloads refuse with restart.transfers_active.
    #[serde(default)]
    pub allow_active: bool,
}

fn value<T: serde::Serialize>(answer: T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(answer)
        .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))
}

#[tool_router(router = system_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List what post-processing can be set to. kind=scripts: the user scripts a category or package may run; plugin_steps: the steps installed plugins provide; upload_destinations: the upload targets installed plugins provide. The names are what update_category_postprocess, update_package and update_collector_package take."
    )]
    pub async fn list_postprocess_options(
        &self,
        Parameters(params): Parameters<PostprocessOptionsParams>,
    ) -> McpToolResult {
        let state = State(self.state.clone());
        let result = async {
            match params.kind {
                PostprocessOptions::Scripts => {
                    value(postprocess::list_postprocess_scripts(state).await?.0)
                }
                PostprocessOptions::PluginSteps => {
                    value(postprocess::list_plugin_steps(state).await.0)
                }
                PostprocessOptions::UploadDestinations => {
                    value(postprocess::list_upload_destinations(state).await.0)
                }
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the post-processing queue: packages waiting to be repaired, unpacked or handed to a script, and the one running now."
    )]
    pub async fn list_postprocess_queue(&self) -> McpToolResult {
        respond(
            postprocess::list_postprocess_queue(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Try the ClamAV scanner the post-processing malware scan uses: PING and VERSION against `address` (host:port or unix:/path), else the saved clamd address. Nothing is scanned. The scan itself is switched with update_settings (malware_scan_enabled, clamd_address) and per category with update_category_postprocess (malware_scan)."
    )]
    pub async fn test_malware_scanner(
        &self,
        Parameters(params): Parameters<MalwareScannerTestParams>,
    ) -> McpToolResult {
        respond(
            postprocess::test_malware_scanner(
                State(self.state.clone()),
                Json(postprocess::MalwareScannerTestRequest {
                    address: params.address,
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Change only the post-processing of one category (id from list_configuration section categories, which also shows the current values). `body` is the REST body of PATCH /api/v1/categories/{id}/postprocess: postprocess_level, script, cleanup_extensions, recursive_unpack, unpack_to_subfolder, direct_unpack, malware_scan, sfv_verify, safe_postproc, delete_par2, plugin_steps, upload_enabled, upload_remote (rclone `remote:path`, or `object-storage:<profile id>/<bucket>/<prefix>` naming an existing profile; one bound to a bucket takes only that bucket), sorting, unwrap_package_folder (dissolve a single folder named like the package, per archive folder too with unpack_to_subfolder), package_name_rules, package_name_regex. Every field is replaced, so pass the current values of the ones you keep. `sorting` is {series, dated, movie}: the sort and rename templates for finished series episodes, dated episodes and films (null = no sorting); try them first with preview_category_sorting. `package_name_rules` is {spaces_to_dots, collapse_separators, strip_bracket_tags, lowercase}, each true, false or null (= the global setting): how the name, and with it the folder, of a new package in this category is tidied; `package_name_regex` is a list of up to 10 {pattern, replacement} pairs run after them (null = the global list, [] = none); try both with preview_package_name. Names come from list_postprocess_options. Naming a script the category does not carry yet is refused with mcp.script_not_allowed unless the person allowed scripts for tools in the settings."
    )]
    pub async fn update_category_postprocess(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let current = super::tools_routing::category(&self.state, id).await?;
            script_gate::check(
                &self.state,
                "script",
                script_gate::body_script(&params.body),
                current.script.as_deref(),
            )
            .await?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(category) = postprocess::update_category_postprocess(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(category)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Preview sort and rename templates on example names before saving them with update_category_postprocess (body.sorting). For each name: what it was recognised as (series: S01E02 / 1x02, multi-episode S01E01E02; dated: 2024.03.15; movie: title and year), the template fields, and the path below the category's folder; a name not recognised, or of a kind without a template, stays where it is. Syntax: {field} or {field:format}, formats 00/000 (pad a number), lower, upper, dots; `/` separates folders, the last part is the file name without extension. Fields: series show, season, episode, title, year, resolution, source; dated show, date, year, month, day, title, resolution, source; movie movie, year, resolution, source. Nothing is saved or moved."
    )]
    pub async fn preview_category_sorting(
        &self,
        Parameters(params): Parameters<SortPreviewParams>,
    ) -> McpToolResult {
        respond(
            postprocess::preview_category_sorting(Json(crate::dto::SortPreviewRequest {
                sorting: rd_core::SortTemplates {
                    series: params.series,
                    dated: params.dated,
                    movie: params.movie,
                },
                names: params.names,
            }))
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Preview the package-name rules (\"Tidy file names\" for package names) on one name: what a new package of that name would be called and the folder it would get. Switches left out, and an omitted regex list, take the saved global setting; pass a category's override to see what it does. The regex pairs run after the switches, in order; a list that does not compile is refused with settings.package_name_regex_invalid. The global switches are the settings key package_name_rules and the global pairs package_name_regex (update_settings); a category's are body.package_name_rules and body.package_name_regex of update_category_postprocess (null inherits, a list replaces the global one). Only a name the application derives is tidied (from a file, an NZB, a torrent, a resolver); a name somebody gave the package stays. Nothing is saved or renamed."
    )]
    pub async fn preview_package_name(
        &self,
        Parameters(params): Parameters<PackageNamePreviewParams>,
    ) -> McpToolResult {
        respond(
            postprocess::preview_package_name(
                State(self.state.clone()),
                Json(crate::dto::PackageNamePreviewRequest {
                    name: params.name,
                    rules: Some(rd_core::PackageNameRulesOverride {
                        spaces_to_dots: params.spaces_to_dots,
                        collapse_separators: params.collapse_separators,
                        strip_bracket_tags: params.strip_bracket_tags,
                        lowercase: params.lowercase,
                    }),
                    regex: params.regex.map(|pairs| {
                        pairs
                            .into_iter()
                            .map(|pair| rd_core::PackageNameRegex {
                                pattern: pair.pattern,
                                replacement: pair.replacement,
                            })
                            .collect()
                    }),
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the managed external tools. view=tools (default): each tool (yt-dlp, ffmpeg, ...), its installed versions, the active one and what the signed manifest offers; view=media: whether yt-dlp and ffmpeg are usable right now and from where."
    )]
    pub async fn list_managed_tools(
        &self,
        Parameters(params): Parameters<ManagedToolsParams>,
    ) -> McpToolResult {
        let state = State(self.state.clone());
        let result = async {
            match params.view.unwrap_or(ManagedToolsView::Tools) {
                ManagedToolsView::Tools => value(tools::list_managed_tools(state).await?.0),
                ManagedToolsView::Media => {
                    value(crate::tools_handlers::media_status(state).await?.0)
                }
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change a managed external tool (name from list_managed_tools). action=install downloads and verifies a version against the signed manifest; activate makes an installed version the one in use; rollback returns to the version active before. `version` is for install and activate; absent means the manifest's current one."
    )]
    pub async fn manage_tool(
        &self,
        Parameters(params): Parameters<ManageToolParams>,
    ) -> McpToolResult {
        let result = async {
            let state = State(self.state.clone());
            let name = Path(params.name);
            let Json(answer) = match params.action {
                ManageAction::Install => {
                    let request = body(serde_json::json!({ "version": params.version }))?;
                    tools::install_managed_tool(state, name, Json(request)).await?
                }
                ManageAction::Activate => {
                    let request = body(serde_json::json!({ "version": params.version }))?;
                    tools::activate_managed_tool(state, name, Json(request)).await?
                }
                ManageAction::Rollback => tools::rollback_managed_tool(state, name).await?,
            };
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Fetch the signed tool manifest again, so list_managed_tools shows what is newly available."
    )]
    pub async fn refresh_tool_manifest(&self) -> McpToolResult {
        respond(
            tools::refresh_tool_manifest(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read free space per storage target: each storage root and the fallback folder, its free and reserved bytes, and whether the service has paused work on it for lack of space."
    )]
    pub async fn get_storage_capacity(&self) -> McpToolResult {
        respond(
            crate::storage_capacity::storage_capacity(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Resume work on a storage target the service paused for lack of space, once space has been made. `target` is a storage root id from get_storage_capacity, or `fallback`."
    )]
    pub async fn resume_storage_target(
        &self,
        Parameters(params): Parameters<StorageTargetParams>,
    ) -> McpToolResult {
        respond(
            crate::storage_capacity::resume_storage(State(self.state.clone()), Path(params.target))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the application update status: the running version, the channel (stable or beta), how this installation was installed, when the last check ran and what it found, and the newer version on offer with its notes for users (one point per line), the links to its full changes (changelog_url) and its release page, and what to do about it (a download, or the package manager's command), whether this installation installs an update itself (installs_itself: the portable archive or the Windows installer) and whether it does so automatically (auto_install, the update_auto_install setting: off by default; once nothing has transferred, post-processed or recorded for five minutes, inside update_auto_install_window if one is set, it installs the offered version with the backup and roll-back of a manual install, announced as update_available before and update_installed or update_failed after), and per connected capture agent its version, whether it is older than the service (outdated) and where its own update stands (self_update: with_service, disabled, unchecked, current, offered, failed or installing; offered_version; remote_update_allowed). An agent installed without the service installs its own update from its tray or with rdownloader-capture update; no tool here installs software on the agent's machine. Read-only; check_for_updates asks GitHub again."
    )]
    pub async fn get_update_status(&self) -> McpToolResult {
        let Json(answer) = updates::get_update_status(State(self.state.clone())).await;
        respond(Ok::<_, ApiError>(answer))
    }

    #[tool(
        description = "Check for a new rDownloader version now, against the signed update manifest of the configured channel, and answer with the same status as get_update_status. Installs nothing. A manifest that is refused or unreachable is reported in error_code."
    )]
    pub async fn check_for_updates(&self) -> McpToolResult {
        respond(
            updates::check_for_updates(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read whether a restart of rDownloader is pending and why: pending, reasons (each a code - plugin_installed, plugin_updated with from_version, plugin_staged, plugin_unstaged, plugin_enabled, plugin_disabled, plugin_removed, plugin_key_revoked, plugin_digest_revoked or plugin_digest_unrevoked - with plugin_id, name and version: a plugin installed or updated by hand or by the automatic plugin update runs only from the next start), how this installation restarts (how: self - rDownloader starts itself again; supervisor - systemd or the container runtime starts it again on exit code 75, supervisor names which, and a container without a restart policy stays stopped; manual - it stops and has to be started by hand), can_restart with blocked_reason (restart.update_running, restart.already_restarting), restarting, automatic (the restart_when_needed setting: off by default; once nothing has transferred, post-processed or recorded for five minutes, inside update_auto_install_window if one is set, the service restarts by itself) and started_at, which changes once a restart is done. Read-only; restart_service carries the restart out."
    )]
    pub async fn get_restart_status(&self) -> McpToolResult {
        let Json(answer) = restarts::get_restart_status(State(self.state.clone())).await;
        respond(Ok::<_, ApiError>(answer))
    }

    #[tool(
        description = "Restart rDownloader now to apply what waits for the next start (get_restart_status says what and how). The service saves its queue, stops and comes back as the same version; this MCP session ends with it, and the agent connects again once get_restart_status answers with a new started_at. Refused with restart.transfers_active while downloads run unless allow_active is true (they are saved by the stop and continue after the restart), with restart.update_running while an update is being installed, restart.already_restarting once a restart began, restart.relaunch_failed when rDownloader could not start its relauncher. Audited as a stop request and announced as the notification event service_restarting."
    )]
    pub async fn restart_service(
        &self,
        Parameters(params): Parameters<RestartParams>,
    ) -> McpToolResult {
        respond(
            restarts::restart_service(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(crate::dto::RestartRequest {
                    allow_active: params.allow_active,
                }),
            )
            .await
            .map(|(_, Json(answer))| answer),
        )
    }

    #[tool(
        description = "Which rDownloader is running: version, commit and build time, the plugin contract versions it links, the platform, its licence and authors, the project's addresses (each says whether it is published yet), and the helper tools the packages ship with their licences."
    )]
    pub async fn get_about(&self) -> McpToolResult {
        let Json(answer) = crate::about_page::system_about(State(self.state.clone())).await;
        respond(Ok::<_, ApiError>(answer))
    }
}
