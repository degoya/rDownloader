# MCP coverage

What the web interface can do, measured against what the MCP toolbox at `/mcp` can do. The unit
is a **capability** — something a person can do — not a REST route: a capability counts as
covered when the toolbox can *do the thing*, not when every route under it has a tool of its own.
Every capability that is left out carries the reason it is left out.

The table is generated, not kept by hand. Its source is `crates/rd-api/src/mcp/coverage.rs`: one
row per capability, the operation counts read from the OpenAPI document the service builds, the
tool names read from `mcp::TOOL_POLICY`. `scripts/mcp-coverage.sh` writes it into this page, and
two tests keep it honest: `coverage::tests` fails `cargo nextest run -p rd-api --lib` when a REST
operation belongs to no capability, so a new route cannot arrive undecided, and
`coverage::doc_tests` fails when this page has drifted from the source.

<!-- BEGIN generated: scripts/mcp-coverage.sh -->

**71 capabilities, 53 covered by a tool, 18 deliberately out (12 of them on the owner's line of 2026-09-23).** 320 REST operations, 164 MCP tools. Regenerate with `scripts/mcp-coverage.sh`; `mcp::coverage` fails the build if an operation belongs to no capability.

### Covered

| Capability | Surface | REST ops | MCP tools |
| --- | --- | --: | --- |
| Queue: add, list and control downloads | Downloads | 4 | `add_downloads`, `control_downloads`, `get_download`, `get_status_summary`, `list_downloads` |
| Packages in the queue | Downloads | 2 | `delete_packages`, `list_packages` |
| LinkGrabber: collect, check, enqueue | LinkGrabber | 5 | `check_links`, `collect_links`, `enqueue_collector`, `list_collector` |
| The settings document | Settings | 2 | `get_settings`, `update_settings` |
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
| Installed plugins: switch and uninstall | Settings > Plugins | 3 | `list_configuration`, `set_plugin_enabled`, `uninstall_plugin_version` |
| Remote jobs | Remote jobs | 4 | `choose_remote_job_entries`, `forget_remote_job`, `list_remote_jobs`, `submit_remote_job` |
| Transfer statistics | Statistics | 1 | `get_transfer_stats` |
| The log store | Logs | 1 | `list_log_records` |
| The audit log | Audit | 1 | `list_audit_records` |
| Clearing logs, audit records and statistics | Settings > System | 4 | `clear_audit_records`, `clear_log_records`, `clear_transfer_stats`, `get_data_reset_preview` |
| Site rules: read and switch | Settings > Site rules | 3 | `list_site_rules`, `set_site_rule_enabled`, `set_site_rule_group_enabled` |
| Handing in a container file | .torrent / .nzb / .dlc drop | 4 | `import_container`, `import_nzb`, `import_torrent` |
| LinkGrabber: candidate-level handling | LinkGrabber | 17 | `clear_linkgrabber`, `delete_candidates`, `enqueue_candidate`, `get_candidate_details`, `list_candidates`, `move_candidates`, `preview_candidate_media`, `reorder_candidates`, `reorder_collector`, `resolve_candidate_torrent`, `set_candidate_plan`, `update_candidate` |
| Mirror groups | LinkGrabber | 5 | `get_mirror_preference`, `set_candidate_mirror`, `set_mirror_preference` |
| Reviewing an NZB before it is queued | NZB import | 1 | `list_nzb_imports` |
| NZB import files and enqueue | NZB import | 4 | `delete_nzb_import`, `enqueue_nzb_import`, `get_nzb_import`, `update_nzb_import` |
| LinkGrabber: package editing and ordering | LinkGrabber | 6 | `delete_collector_package`, `regroup_collector`, `update_collector_package`, `update_collector_packages` |
| Ordering the queue by hand | Downloads | 2 | `reorder_downloads`, `reorder_packages` |
| Renaming and retargeting queued work | Downloads | 5 | `rename_download`, `rename_package_folder`, `update_package`, `update_packages` |
| Clearing finished work in one sweep | Downloads | 1 | `clear_finished_packages` |
| Unpacking on demand | Downloads | 4 | `extract_downloads`, `extract_packages` |
| Torrent detail and seeding | Downloads > torrent panel | 18 | `get_torrent_details`, `get_torrent_engine`, `list_network_interfaces`, `set_category_seeding`, `set_torrent_file_plan`, `set_torrent_seeding`, `stop_seeding`, `update_torrent_trackers` |
| Post-processing inventory and queue | Settings > Post-processing | 7 | `get_nzb_import`, `get_package_postprocess`, `list_postprocess_options`, `list_postprocess_queue`, `update_category_postprocess` |
| Managed external tools | Settings > Tools | 6 | `list_managed_tools`, `manage_tool`, `refresh_tool_manifest` |
| Storage capacity | Settings > Storage | 2 | `get_storage_capacity`, `resume_storage_target` |
| About rDownloader | Settings > About | 2 | `get_about` |
| Writing a site rule | Settings > Site rules | 4 | `create_site_rule`, `delete_site_rule`, `test_site_rule`, `update_site_rule` |
| Which providers can take a remote job | Remote jobs | 1 | `list_remote_job_providers` |
| Power actions | Settings > Power | 2 | `cancel_power_action`, `get_power_status` |
| Plugin execution history | Settings > Plugins | 1 | `list_plugin_executions` |
| Plugin message catalogues | the interface itself | 1 | `get_plugin_messages` |
| Automation history, vocabulary and dry run | Automation | 4 | `dry_run_automations`, `get_automation_vocabulary`, `list_automation_runs`, `list_automation_versions` |
| Notification history and the destination catalogue | Settings > Notifications | 2 | `list_notification_deliveries`, `list_notification_destinations` |
| Clearing the notification history | Settings > Notifications | 1 | `clear_notification_deliveries` |
| Subscription items, runs and forced polls | Subscriptions | 10 | `clear_subscription_history`, `get_subscription_review_summary`, `list_subscription_items`, `list_subscription_runs`, `poll_subscription`, `review_pending_subscription_items`, `review_subscription_item`, `set_subscription_enabled` |
| Stream schedules, runs and recording now | Streams | 6 | `create_stream_schedule`, `delete_stream_schedule`, `list_stream_runs`, `list_stream_schedules`, `record_stream_now`, `update_stream_schedule` |
| The diagnostic bundle: preview | Logs | 1 | `preview_diagnostic_bundle` |
| Metrics | - | 1 | `get_metrics` |
| Reconnect status | Settings > Network | 1 | `get_reconnect_status` |
| The hosters one account covers | Settings > Accounts | 1 | `list_account_hosters` |
| Trying a routing regular expression | Settings > Routing | 1 | `test_category_regex` |

### Deliberately out

| Capability | Surface | REST ops | Why not |
| --- | --- | --: | --- |
| Deleting a remote job at the provider | Remote jobs | 1 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Signing in, sessions, second factor and API tokens | Login, Settings > Security | 25 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Signing in at a provider | Settings > Accounts | 13 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Trying a stored credential or destination | several forms | 6 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Remote logins and trusted host keys | Settings > Remote | 7 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Solving captchas | captcha dialog | 7 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Consent to replay a paid link | LinkGrabber | 3 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Import and export of a whole area | Settings > Backup | 14 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Plugin trust and installation | Settings > Plugins | 6 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Probing an indexer's capabilities | Subscriptions | 2 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Approving and fetching a diagnostic bundle | Logs | 2 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Reconnecting on demand | Settings > Network | 1 | Owner's decision, 2026-09-23 (RD-120-32): not offered. A tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered -- not because it could not be built, but because an agent holding it could do what the person meant to do themselves. |
| Choosing a stored browser profile for queued work | Downloads, LinkGrabber | 2 | Each route names one of the stored browser profiles, and listing those is part of signing in at a provider, which the owner decided on 2026-09-23 to keep out. A tool here would take an id no tool can supply -- the gap RD-120-32 exists to close, not one to open. |
| The desktop capture agent | the agent, not the web UI | 15 | Not a user-facing capability but the agent's own contract, priced with its own capture: scope. No api: token reaches it, so a tool over it could not be called. |
| Controlling one download by its own route | Downloads | 5 | control_downloads already does all five for one id or many, over the bulk route. A second spelling of the same act is one more thing for a model to choose between and nothing it could not do before. |
| The live rate series | Downloads chart | 1 | A chart's data series, sampled per second. get_status_summary answers how fast the queue is going in one number, and get_transfer_stats answers it over time. |
| Bandwidth budgets and quiet hours | Settings > Bandwidth | 8 | The limit in force is in the settings document, which update_settings writes. Profiles and the weekly schedule are a calendar grid, and a schedule edited by something that cannot see it is how a quiet hour lands on the wrong day. |
| The health probe | - | 1 | Public by design: a load balancer asks it without a token, so no permission prices it, and mcp::tool_scope refuses a tool priced by a public route rather than making it free. What it answers -- the service is up, its name and version -- is what the MCP initialize handshake already carries in its server_info. |

<!-- END generated -->
