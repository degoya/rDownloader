# MCP coverage

What the web interface can do, measured against what the MCP toolbox at `/mcp` can do. The unit
is a **capability** — something a person can do — not a REST route: a capability counts as
covered when the toolbox can *do the thing*, not when every route under it has a tool of its own.
Every capability that is left out carries the reason it is left out.

The table is generated, not kept by hand. Its source is `crates/rd-api/src/mcp_coverage.rs`: one
row per capability, the operation counts read from the OpenAPI document the service builds, the
tool names read from `rd_api_mcp::TOOL_POLICY`. `scripts/mcp-coverage.sh` writes it into this
page, and two tests keep it honest: `mcp_coverage::tests` fails `cargo nextest run -p rd-api
--lib` when a REST operation belongs to no capability, so a new route cannot arrive undecided,
and `mcp_coverage::doc_tests` fails when this page has drifted from the source.

<!-- BEGIN generated: scripts/mcp-coverage.sh -->

**104 capabilities, 79 covered by a tool, 25 deliberately out (16 of them on the owner's line of 2026-09-23).** 447 REST operations, 239 MCP tools. Regenerate with `scripts/mcp-coverage.sh`; `mcp::coverage` fails the build if an operation belongs to no capability.

### Covered

| Capability | Surface | REST ops | MCP tools |
| --- | --- | --: | --- |
| Queue: add, list and control downloads | Downloads | 4 | `add_downloads`, `control_downloads`, `get_download`, `get_status_summary`, `list_downloads` |
| Packages in the queue | Downloads | 2 | `delete_packages`, `list_packages` |
| LinkGrabber: collect, check, enqueue | LinkGrabber | 5 | `check_links`, `collect_links`, `enqueue_collector`, `list_collector` |
| The settings document | Settings | 2 | `get_settings`, `update_settings` |
| The desktop agent's clipboard pause and shortcuts | Settings > Clients & API > Desktop | 2 | `get_capture_agent_settings`, `update_capture_agent_settings` |
| Full backup: schedule, runs and history | Settings > Backup | 4 | `get_backup_status`, `list_backup_runs`, `run_backup`, `update_backup_schedule` |
| Full backup: destinations, retention and verification | Settings > Backup | 7 | `create_backup_destination`, `delete_backup_destination`, `list_backup_archives`, `list_backup_verifications`, `preview_backup_retention`, `update_backup_destination`, `verify_backup_archive` |
| Categories | Settings > Routing | 4 | `create_category`, `delete_category`, `list_configuration`, `update_category` |
| Routing rules | Settings > Routing | 4 | `create_category_rule`, `delete_category_rule`, `list_category_rules`, `update_category_rule` |
| Storage roots | Settings > Storage | 4 | `create_storage_root`, `delete_storage_root`, `list_configuration`, `update_storage_root` |
| Watched folders | Settings > Intake | 4 | `create_hotfolder`, `delete_hotfolder`, `list_hotfolders`, `update_hotfolder` |
| Provider accounts | Settings > Accounts | 4 | `create_account`, `delete_account`, `list_configuration`, `update_account` |
| The provider table | Settings > Accounts | 1 | `list_configuration` |
| Proxy profiles | Settings > Network | 4 | `create_proxy_profile`, `delete_proxy_profile`, `list_configuration`, `update_proxy_profile` |
| Usenet servers | Settings > Usenet | 4 | `create_usenet_server`, `delete_usenet_server`, `list_usenet_servers`, `update_usenet_server` |
| Notification targets and rules | Settings > Notifications | 8 | `create_notification_rule`, `create_notification_target`, `delete_notification_rule`, `delete_notification_target`, `list_notification_rules`, `list_notification_targets`, `update_notification_rule`, `update_notification_target` |
| Subscriptions | Subscriptions | 4 | `create_subscription`, `delete_subscription`, `list_subscriptions`, `update_subscription` |
| Livestream channels | Streams | 4 | `create_stream_channel`, `delete_stream_channel`, `list_stream_channels`, `update_stream_channel` |
| Automations | Automation | 5 | `create_automation`, `delete_automation`, `list_automations`, `toggle_automation`, `update_automation` |
| Installed plugins: switch and uninstall | Settings > Plugins | 5 | `list_configuration`, `remove_superseded_plugin_versions`, `set_plugin_enabled`, `uninstall_plugin_version` |
| Choosing the bundled services | Setup wizard, Settings > Plugins | 3 | `install_bundled_services`, `list_bundled_services`, `remove_bundled_services` |
| Remote jobs | Remote jobs | 7 | `choose_remote_job_entries`, `clear_remote_jobs`, `forget_remote_job`, `list_remote_jobs`, `submit_nzb_import_remote_job`, `submit_package_remote_job`, `submit_remote_job` |
| Transfer statistics | Statistics | 1 | `get_transfer_stats` |
| Traffic per Usenet server and its quota | Statistics, Settings > Usenet | 2 | `get_usenet_server_traffic`, `set_usenet_server_quota` |
| The log store | Logs | 1 | `list_log_records` |
| The audit log | Audit | 1 | `list_audit_records` |
| Clearing logs, audit records and statistics | Settings > System | 4 | `clear_audit_records`, `clear_log_records`, `clear_transfer_stats`, `get_data_reset_preview` |
| Site rules: read and switch | Settings > Site rules | 3 | `list_site_rules`, `set_site_rule_enabled`, `set_site_rule_group_enabled` |
| Handing in a container file | .torrent / .nzb / .dlc / .rdlinks / .crawljob drop | 4 | `import_container`, `import_nzb`, `import_torrent` |
| LinkGrabber: candidate-level handling | LinkGrabber | 17 | `clear_linkgrabber`, `delete_candidates`, `enqueue_candidate`, `get_candidate_details`, `list_candidates`, `move_candidates`, `preview_candidate_media`, `reorder_candidates`, `reorder_collector`, `resolve_candidate_torrent`, `set_candidate_plan`, `update_candidate` |
| Mirror groups | LinkGrabber | 5 | `get_mirror_preference`, `set_candidate_mirror`, `set_mirror_preference` |
| Reviewing an NZB before it is queued | NZB import | 1 | `list_nzb_imports` |
| NZB import files and enqueue | NZB import | 4 | `delete_nzb_import`, `enqueue_nzb_import`, `get_nzb_import`, `update_nzb_import` |
| LinkGrabber: package editing and ordering | LinkGrabber | 6 | `delete_collector_package`, `regroup_collector`, `update_collector_package`, `update_collector_packages` |
| Ordering the queue by hand | Downloads | 2 | `reorder_downloads`, `reorder_packages` |
| Renaming and retargeting queued work | Downloads | 5 | `rename_download`, `rename_package_folder`, `update_package`, `update_packages` |
| Clearing finished work in one sweep | Downloads | 1 | `clear_finished_packages` |
| Unpacking on demand | Downloads | 4 | `extract_downloads`, `extract_packages` |
| The mirrors of a download and their health | Downloads > transfer details | 1 | `get_download_sources` |
| Torrent detail and seeding | Downloads > torrent panel | 18 | `get_torrent_details`, `get_torrent_engine`, `list_network_interfaces`, `set_category_seeding`, `set_torrent_file_plan`, `set_torrent_seeding`, `stop_seeding`, `update_torrent_trackers` |
| Rechecking a torrent and moving its files | Downloads > torrent menu | 2 | `move_torrent`, `recheck_torrent` |
| Post-processing inventory and queue | Settings > Post-processing | 8 | `get_nzb_import`, `get_package_postprocess`, `list_postprocess_options`, `list_postprocess_queue`, `test_malware_scanner`, `update_category_postprocess` |
| Sort and rename templates for series and films | Settings > Routing > category | 1 | `preview_category_sorting` |
| Package-name rules, global and per category | Settings > Post-processing; Settings > Routing > category | 1 | `preview_package_name` |
| Managed external tools | Settings > Tools | 6 | `list_managed_tools`, `manage_tool`, `refresh_tool_manifest` |
| Storage capacity | Settings > Storage | 2 | `get_storage_capacity`, `resume_storage_target` |
| File collision policies | Settings > General, Settings > Routing, package editor | 4 | `get_package_collision_policy`, `list_collision_policies`, `set_category_collision_policy`, `set_package_collision_policy` |
| Answering a collision prompt | Downloads | 2 | `decide_collision`, `list_collision_prompts` |
| Source and content duplicates | Downloads > package, LinkGrabber | 3 | `dedupe_download`, `get_download_duplicates`, `lookup_duplicates` |
| Storage history, reuse and the content index | Settings > Storage | 4 | `check_content_index`, `get_link_support`, `get_storage_reuse`, `list_storage_operations` |
| Clearing the storage history and the content index | Settings > Storage | 2 | `clear_content_index`, `clear_storage_operations` |
| Download history: search, add again, clear | History | 3 | `clear_download_history`, `list_download_history`, `readd_history_entry` |
| About rDownloader | Settings > About | 2 | `get_about` |
| Application updates | Settings > System | 2 | `check_for_updates`, `get_update_status` |
| Writing a site rule | Settings > Site rules | 4 | `create_site_rule`, `delete_site_rule`, `test_site_rule`, `update_site_rule` |
| Choosing a series page's releases before resolving them | LinkGrabber | 6 | `cancel_page_pick`, `discard_page_pick`, `get_page_pick`, `list_page_entries`, `list_page_picks`, `resolve_page_entries` |
| Which providers can take a remote job | Remote jobs | 1 | `list_remote_job_providers` |
| Power actions | Settings > Power | 2 | `cancel_power_action`, `get_power_status` |
| Plugin execution history | Settings > Plugins | 1 | `list_plugin_executions` |
| Plugin updates and repository offers | Settings > Plugins | 1 | `list_plugin_updates` |
| Automatic updates for all plugins | Settings > Plugins | 2 | `get_plugin_update_settings`, `set_plugin_update_settings` |
| Plugin message catalogues | the interface itself | 1 | `get_plugin_messages` |
| Automation history, vocabulary and dry run | Automation | 4 | `dry_run_automations`, `get_automation_vocabulary`, `list_automation_runs`, `list_automation_versions` |
| Notification history and the destination catalogue | Settings > Notifications | 2 | `list_notification_deliveries`, `list_notification_destinations` |
| Clearing the notification history and discarding pending notifications | Settings > Notifications | 2 | `clear_notification_deliveries`, `discard_pending_notification_deliveries` |
| Subscription items, runs and forced polls | Subscriptions | 10 | `clear_subscription_history`, `get_subscription_review_summary`, `list_subscription_items`, `list_subscription_runs`, `poll_subscription`, `requeue_subscription_items`, `review_pending_subscription_items`, `review_subscription_item`, `set_subscription_enabled` |
| Searching Newznab and Torznab indexers and taking hits into the LinkGrabber | LinkGrabber > Indexer search | 3 | `grab_indexer_results`, `list_indexers`, `search_indexers` |
| Stream schedules, runs and recording now | Streams | 6 | `create_stream_schedule`, `delete_stream_schedule`, `list_stream_runs`, `list_stream_schedules`, `record_stream_now`, `update_stream_schedule` |
| The diagnostic bundle: preview | Logs | 1 | `preview_diagnostic_bundle` |
| Metrics | - | 1 | `get_metrics` |
| Reconnect status | Settings > Network | 1 | `get_reconnect_status` |
| The hosters one account covers | Settings > Accounts | 1 | `list_account_hosters` |
| Trying a routing regular expression | Settings > Routing | 1 | `test_category_regex` |
| Pausing the whole queue for a while | Downloads, transfer rail | 3 | `get_queue_pause`, `pause_queue`, `resume_queue` |
| Switching a bandwidth profile by hand, and the bandwidth status | Settings > Bandwidth | 4 | `get_bandwidth_status`, `list_bandwidth_profiles`, `return_to_bandwidth_schedule`, `switch_bandwidth_profile` |
| A package's own speed limit | Downloads > package editor | 2 | `get_package_speed_limit`, `set_package_speed_limit` |
| Exporting packages as a link file | Downloads and LinkGrabber, selection bar and package menu | 1 | `export_packages` |
| Resolving downloads again with the plugin installed now | Downloads, selection bar and package menu | 1 | `reresolve_downloads` |
| The queue's stop mark | Downloads, row menu and transfer rail | 3 | `clear_stop_mark`, `get_queue_pause`, `set_stop_mark` |

### Deliberately out

| Capability | Surface | REST ops | Why not |
| --- | --- | --: | --- |
| Deleting a remote job at the provider | Remote jobs | 1 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Signing in, sessions, second factor and API tokens | Login, Settings > Security | 35 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Signing in at a provider | Settings > Accounts | 13 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Trying a stored credential or destination | several forms | 7 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Remote logins and trusted host keys | Settings > Remote | 7 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Object storage profiles | Settings > Transfers | 4 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Solving captchas | captcha dialog | 7 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Consent to replay a paid link | LinkGrabber | 3 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Full backup passphrase | Settings > Backup | 1 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Restoring a full backup | Settings > Backup | 8 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Import and export of a whole area | Settings > Backup | 14 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Plugin trust and installation | Settings > Plugins | 21 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Probing an indexer's capabilities | Subscriptions | 2 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Defining and testing indexers | Settings > Usenet | 4 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Approving and fetching a diagnostic bundle | Logs | 2 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Reconnecting on demand | Settings > Network | 1 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Choosing a stored browser profile for queued work | Downloads, LinkGrabber | 2 | Each route names one of the stored browser profiles, and listing those is part of signing in at a provider, which the owner decided on 2026-09-23 to keep out. A tool here would take an id no tool can supply -- the gap RD-120-32 exists to close, not one to open. |
| The desktop capture agent | the agent, not the web UI | 20 | Not a user-facing capability but the agent's own contract, priced with its own capture: scopes. No api: token reaches it, so a tool over it could not be called. The tray's pause and resume (RD-1100-06) are the capability pause_queue and resume_queue already give MCP, and the clipboard pause and the shortcuts the agent follows (RD-1180-01, RD-1180-03) the one get_ and update_capture_agent_settings give it. What an agent says about its own update on its poll (RD-1210-03) get_update_status reads; installing it is the agent's own decision, never a tool's. |
| Controlling one download by its own route | Downloads | 5 | control_downloads already does all five for one id or many, over the bulk route. A second spelling of the same act is one more thing for a model to choose between and nothing it could not do before. |
| The live rate series | Downloads chart | 1 | A chart's data series, sampled per second. get_status_summary answers how fast the queue is going in one number, and get_transfer_stats answers it over time; the queued files waiting for their host are list_downloads' waiting_for_host. |
| Editing bandwidth profiles and the weekly schedule | Settings > Bandwidth | 6 | The limit in force is in the settings document, which update_settings writes. Profiles and the weekly schedule are a calendar grid, and a schedule edited by something that cannot see it is how a quiet hour lands on the wrong day. Reading the status, listing the profiles and switching one on for a while are tools (RD-190-20). |
| The health probe | - | 1 | Public by design: a load balancer asks it without a token, so no permission prices it, and mcp::tool_scope refuses a tool priced by a public route rather than making it free. What it answers -- the service is up, its name and version -- is what the MCP initialize handshake already carries in its server_info. |
| Stopping the service and the backup before an update | - | 2 | Refused from anywhere but the machine the service runs on, and meant for the launchers and the updater there. A tool that stops the service ends the MCP session that called it, and the backup before an update is the first step of a version switch that no agent performs. |
| Setting a new administrator password without the current one | - | 1 | rdownloader auth reset-password on the machine the service runs on, and nothing else (RD-190-24): only the local control token opens the route, from this machine. Whoever holds the data directory may recover the installation; an agent that merely reaches the service must not be able to replace the password and end every session. |
| Installing an update | Settings > System | 2 | Installing stops the service, replaces its program and starts it again: the MCP session that asked ends with the process, and a version switch is the administrator's decision in the interface, not an agent's. Downloading the update ahead of it is the first step of that install and nothing else. get_update_status shows what a download or an install is doing. |

<!-- END generated -->
