//! Which REST route each MCP tool pays for (RD-101-16, RD-120-32).
//!
//! Split out of `mod.rs` when RD-120-32 doubled the toolbox: the table is the part that grows
//! with every tool, and the enforcement beside it does not.

use axum::http::Method;

/// One tool and the REST route whose price it pays.
pub(super) struct ToolPolicy {
    pub tool: &'static str,
    /// The registered axum pattern the tool reaches, as [`crate::scope_policy`] spells it.
    pub path: &'static str,
    pub method: Method,
}

const fn tool(tool: &'static str, path: &'static str, method: Method) -> ToolPolicy {
    ToolPolicy { tool, path, method }
}

/// Which scope each tool costs — named as the route it delegates to, not as a scope.
///
/// The tools are as capable as the REST routes behind them — `update_settings` writes the
/// settings document, `delete_packages` deletes work, `delete_category` removes a category —
/// but until RD-101-16 they sat behind one blanket `api:*` check, so handing an assistant the
/// ability to see the queue meant handing it the whole installation.
///
/// The table names the *route*, and the price is read from [`crate::scope_policy`]. Writing
/// the scope here instead would be a second copy of a decision that already exists, free to
/// drift below the endpoint it calls; naming the route makes "a tool is never cheaper than the
/// endpoint underneath it" true by construction rather than by review.
/// `tests::the_tool_policy_and_the_router_describe_the_same_tools` keeps the table and the
/// router in step in both directions: a tool added without an entry fails the build rather
/// than defaulting to anything.
#[rustfmt::skip]
pub(super) const TOOL_POLICY: &[ToolPolicy] = &[
    tool("add_downloads", "/api/v1/downloads", Method::POST),
    tool("cancel_power_action", "/api/v1/power/cancel", Method::POST),
    tool("check_links", "/api/v1/collector/candidates/check", Method::POST),
    tool("choose_remote_job_entries", "/api/v1/remote-jobs/{id}/choice", Method::POST),
    tool("clear_audit_records", "/api/v1/audit/records/clear", Method::POST),
    tool("clear_finished_packages", "/api/v1/packages/clear", Method::POST),
    tool("clear_linkgrabber", "/api/v1/collector/candidates", Method::DELETE),
    tool("clear_log_records", "/api/v1/diagnostics/logs/clear", Method::POST),
    tool("clear_notification_deliveries", "/api/v1/notifications/deliveries/clear", Method::POST),
    tool("clear_subscription_history", "/api/v1/subscriptions/{id}/history", Method::DELETE),
    tool("clear_transfer_stats", "/api/v1/stats/transfers/clear", Method::POST),
    tool("collect_links", "/api/v1/collector/batches", Method::POST),
    tool("control_downloads", "/api/v1/downloads/bulk", Method::POST),
    tool("create_account", "/api/v1/accounts", Method::POST),
    tool("create_automation", "/api/v1/automations", Method::POST),
    tool("create_category", "/api/v1/categories", Method::POST),
    tool("create_category_rule", "/api/v1/category-rules", Method::POST),
    tool("create_hotfolder", "/api/v1/hotfolders", Method::POST),
    tool("create_notification_rule", "/api/v1/notifications/rules", Method::POST),
    tool("create_notification_target", "/api/v1/notifications/targets", Method::POST),
    tool("create_proxy_profile", "/api/v1/proxy-profiles", Method::POST),
    tool("create_site_rule", "/api/v1/site-rules", Method::POST),
    tool("create_storage_root", "/api/v1/storage-roots", Method::POST),
    tool("create_stream_channel", "/api/v1/streams/channels", Method::POST),
    tool("create_stream_schedule", "/api/v1/streams/schedules", Method::POST),
    tool("create_subscription", "/api/v1/subscriptions", Method::POST),
    tool("create_usenet_server", "/api/v1/usenet/servers", Method::POST),
    tool("delete_account", "/api/v1/accounts/{id}", Method::DELETE),
    tool("delete_automation", "/api/v1/automations/{id}", Method::DELETE),
    tool("delete_candidates", "/api/v1/collector/candidates/{id}", Method::DELETE),
    tool("delete_category", "/api/v1/categories/{id}", Method::DELETE),
    tool("delete_category_rule", "/api/v1/category-rules/{id}", Method::DELETE),
    tool("delete_collector_package", "/api/v1/collector/packages/{id}", Method::DELETE),
    tool("delete_hotfolder", "/api/v1/hotfolders/{id}", Method::DELETE),
    tool("delete_notification_rule", "/api/v1/notifications/rules/{id}", Method::DELETE),
    tool("delete_notification_target", "/api/v1/notifications/targets/{id}", Method::DELETE),
    tool("delete_nzb_import", "/api/v1/nzb/imports/{id}", Method::DELETE),
    tool("delete_packages", "/api/v1/packages/delete", Method::POST),
    tool("delete_proxy_profile", "/api/v1/proxy-profiles/{id}", Method::DELETE),
    tool("delete_site_rule", "/api/v1/site-rules/{id}", Method::DELETE),
    tool("delete_storage_root", "/api/v1/storage-roots/{id}", Method::DELETE),
    tool("delete_stream_channel", "/api/v1/streams/channels/{id}", Method::DELETE),
    tool("delete_stream_schedule", "/api/v1/streams/schedules/{id}", Method::DELETE),
    tool("delete_subscription", "/api/v1/subscriptions/{id}", Method::DELETE),
    tool("delete_usenet_server", "/api/v1/usenet/servers/{id}", Method::DELETE),
    tool("dry_run_automations", "/api/v1/automations/dry-run", Method::POST),
    tool("enqueue_candidate", "/api/v1/collector/candidates/{id}/enqueue", Method::POST),
    tool("enqueue_collector", "/api/v1/collector/packages/enqueue", Method::POST),
    tool("enqueue_nzb_import", "/api/v1/nzb/imports/{id}/enqueue", Method::POST),
    tool("extract_downloads", "/api/v1/downloads/extract", Method::POST),
    tool("extract_packages", "/api/v1/packages/extract", Method::POST),
    tool("forget_remote_job", "/api/v1/remote-jobs/{id}", Method::DELETE),
    tool("get_about", "/api/v1/system/about", Method::GET),
    tool("get_automation_vocabulary", "/api/v1/automations/vocabulary", Method::GET),
    tool("get_candidate_details", "/api/v1/collector/candidates/{id}/media", Method::GET),
    tool("get_data_reset_preview", "/api/v1/system/data-reset", Method::GET),
    tool("get_download", "/api/v1/downloads", Method::GET),
    tool("get_metrics", "/api/v1/metrics", Method::GET),
    tool("get_mirror_preference", "/api/v1/collector/mirror-preference", Method::GET),
    tool("get_nzb_import", "/api/v1/nzb/imports/{id}/files", Method::GET),
    tool("get_package_postprocess", "/api/v1/packages/{id}/postprocess", Method::GET),
    tool("get_plugin_messages", "/api/v1/plugins/i18n/{locale}", Method::GET),
    tool("get_power_status", "/api/v1/power/status", Method::GET),
    tool("get_reconnect_status", "/api/v1/reconnect", Method::GET),
    tool("get_settings", "/api/v1/settings", Method::GET),
    tool("get_status_summary", "/api/v1/downloads/summary", Method::GET),
    tool("get_storage_capacity", "/api/v1/storage/capacity", Method::GET),
    tool("get_subscription_review_summary", "/api/v1/subscriptions/review-summary", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent", Method::GET),
    tool("get_torrent_engine", "/api/v1/torrents/capabilities", Method::GET),
    tool("get_transfer_stats", "/api/v1/stats/transfers", Method::GET),
    tool("import_container", "/api/v1/containers/import", Method::POST),
    tool("import_nzb", "/api/v1/nzb/imports", Method::POST),
    tool("import_torrent", "/api/v1/torrents/import", Method::POST),
    tool("list_account_hosters", "/api/v1/accounts/{id}/hosters", Method::GET),
    tool("list_audit_records", "/api/v1/audit/records", Method::GET),
    tool("list_automation_runs", "/api/v1/automations/runs", Method::GET),
    tool("list_automation_versions", "/api/v1/automations/{id}/versions", Method::GET),
    tool("list_automations", "/api/v1/automations", Method::GET),
    tool("list_candidates", "/api/v1/collector/candidates", Method::GET),
    tool("list_category_rules", "/api/v1/category-rules", Method::GET),
    tool("list_collector", "/api/v1/collector/packages", Method::GET),
    // The cheapest section it can read. The dearer ones are priced per section by
    // `section_scope`, because plugins are administration and accounts are credentials, and
    // no single scope confers both.
    tool("list_configuration", "/api/v1/categories", Method::GET),
    tool("list_downloads", "/api/v1/downloads", Method::GET),
    tool("list_hotfolders", "/api/v1/hotfolders", Method::GET),
    tool("list_log_records", "/api/v1/diagnostics/logs", Method::GET),
    tool("list_managed_tools", "/api/v1/system/tools", Method::GET),
    tool("list_network_interfaces", "/api/v1/torrents/network/interfaces", Method::GET),
    tool("list_notification_deliveries", "/api/v1/notifications/deliveries", Method::GET),
    tool("list_notification_destinations", "/api/v1/notifications/destinations", Method::GET),
    tool("list_notification_rules", "/api/v1/notifications/rules", Method::GET),
    tool("list_notification_targets", "/api/v1/notifications/targets", Method::GET),
    tool("list_nzb_imports", "/api/v1/nzb/imports", Method::GET),
    tool("list_packages", "/api/v1/packages", Method::GET),
    tool("list_plugin_executions", "/api/v1/plugins/{id}/executions", Method::GET),
    tool("list_postprocess_options", "/api/v1/postprocess/scripts", Method::GET),
    tool("list_postprocess_queue", "/api/v1/postprocess/queue", Method::GET),
    tool("list_remote_job_providers", "/api/v1/remote-jobs/providers", Method::GET),
    tool("list_remote_jobs", "/api/v1/remote-jobs", Method::GET),
    tool("list_site_rules", "/api/v1/site-rules", Method::GET),
    tool("list_stream_channels", "/api/v1/streams/channels", Method::GET),
    tool("list_stream_runs", "/api/v1/streams/runs", Method::GET),
    tool("list_stream_schedules", "/api/v1/streams/schedules", Method::GET),
    tool("list_subscription_items", "/api/v1/subscriptions/{id}/items/page", Method::GET),
    tool("list_subscription_runs", "/api/v1/subscriptions/{id}/runs", Method::GET),
    tool("list_subscriptions", "/api/v1/subscriptions", Method::GET),
    tool("list_usenet_servers", "/api/v1/usenet/servers", Method::GET),
    tool("manage_tool", "/api/v1/system/tools/{name}/install", Method::POST),
    tool("move_candidates", "/api/v1/collector/candidates/move", Method::POST),
    tool("poll_subscription", "/api/v1/subscriptions/{id}/poll", Method::POST),
    tool("preview_candidate_media", "/api/v1/collector/candidates/{id}/media/preview", Method::POST),
    tool("preview_diagnostic_bundle", "/api/v1/diagnostics/bundle/preview", Method::GET),
    tool("record_stream_now", "/api/v1/streams/record", Method::POST),
    tool("refresh_tool_manifest", "/api/v1/system/tools/manifest/refresh", Method::POST),
    tool("regroup_collector", "/api/v1/collector/packages/regroup", Method::POST),
    tool("rename_download", "/api/v1/downloads/{id}", Method::PATCH),
    tool("rename_package_folder", "/api/v1/packages/{id}/folder", Method::POST),
    tool("reorder_candidates", "/api/v1/collector/candidates/reorder", Method::POST),
    tool("reorder_collector", "/api/v1/collector/entries/reorder", Method::POST),
    tool("reorder_downloads", "/api/v1/downloads/reorder", Method::POST),
    tool("reorder_packages", "/api/v1/packages/reorder", Method::POST),
    tool("resolve_candidate_torrent", "/api/v1/collector/candidates/{id}/torrent/resolve", Method::POST),
    tool("resume_storage_target", "/api/v1/storage/capacity/{target}/resume", Method::POST),
    tool("review_pending_subscription_items", "/api/v1/subscriptions/{id}/items/pending", Method::PUT),
    tool("review_subscription_item", "/api/v1/subscriptions/items/{id}", Method::PUT),
    tool("set_candidate_mirror", "/api/v1/collector/candidates/{id}/mirror", Method::POST),
    tool("set_candidate_plan", "/api/v1/collector/candidates/{id}/media/selection", Method::PUT),
    tool("set_category_seeding", "/api/v1/categories/{id}/seeding", Method::PUT),
    tool("set_mirror_preference", "/api/v1/collector/mirror-preference", Method::PUT),
    tool("set_plugin_enabled", "/api/v1/plugins/{id}", Method::PATCH),
    tool("set_site_rule_enabled", "/api/v1/site-rules/{id}/enabled", Method::PUT),
    tool("set_site_rule_group_enabled", "/api/v1/site-rule-groups/{group}/enabled", Method::PUT),
    tool("set_subscription_enabled", "/api/v1/subscriptions/{id}/enable", Method::POST),
    tool("set_torrent_file_plan", "/api/v1/downloads/{id}/torrent/plan", Method::PUT),
    tool("set_torrent_seeding", "/api/v1/downloads/{id}/torrent/seeding", Method::PUT),
    tool("stop_seeding", "/api/v1/downloads/{id}/seeding/stop", Method::POST),
    tool("submit_remote_job", "/api/v1/accounts/{id}/remote-jobs", Method::POST),
    tool("test_category_regex", "/api/v1/category-rules/test-regex", Method::POST),
    tool("test_site_rule", "/api/v1/site-rules/test", Method::POST),
    tool("toggle_automation", "/api/v1/automations/{id}/enable", Method::POST),
    tool("uninstall_plugin_version", "/api/v1/plugins/{id}/{version}", Method::DELETE),
    tool("update_account", "/api/v1/accounts/{id}", Method::PUT),
    tool("update_automation", "/api/v1/automations/{id}", Method::PUT),
    tool("update_candidate", "/api/v1/collector/candidates/{id}", Method::PATCH),
    tool("update_category", "/api/v1/categories/{id}", Method::PUT),
    tool("update_category_postprocess", "/api/v1/categories/{id}/postprocess", Method::PATCH),
    tool("update_category_rule", "/api/v1/category-rules/{id}", Method::PUT),
    tool("update_collector_package", "/api/v1/collector/packages/{id}", Method::PATCH),
    tool("update_collector_packages", "/api/v1/collector/packages/bulk", Method::POST),
    tool("update_hotfolder", "/api/v1/hotfolders/{id}", Method::PUT),
    tool("update_notification_rule", "/api/v1/notifications/rules/{id}", Method::PUT),
    tool("update_notification_target", "/api/v1/notifications/targets/{id}", Method::PUT),
    tool("update_nzb_import", "/api/v1/nzb/imports/{id}", Method::PATCH),
    tool("update_package", "/api/v1/packages/{id}", Method::PATCH),
    tool("update_packages", "/api/v1/packages/bulk", Method::POST),
    tool("update_proxy_profile", "/api/v1/proxy-profiles/{id}", Method::PUT),
    tool("update_settings", "/api/v1/settings", Method::PUT),
    tool("update_site_rule", "/api/v1/site-rules/{id}", Method::PUT),
    tool("update_storage_root", "/api/v1/storage-roots/{id}", Method::PUT),
    tool("update_stream_channel", "/api/v1/streams/channels/{id}", Method::PUT),
    tool("update_stream_schedule", "/api/v1/streams/schedules/{id}", Method::PUT),
    tool("update_subscription", "/api/v1/subscriptions/{id}", Method::PUT),
    tool("update_torrent_trackers", "/api/v1/downloads/{id}/torrent/trackers", Method::PUT),
    tool("update_usenet_server", "/api/v1/usenet/servers/{id}", Method::PUT),
];

/// The further routes a tool reaches besides the one [`TOOL_POLICY`] prices it by.
///
/// RD-120-32 gave some tools a `view` or an `action` argument rather than one tool per route —
/// the six read-outs of a torrent's panel are one question asked six ways, and six tools for it
/// would make every call a choice between spellings. Such a tool is still priced by one route,
/// so every other route it reaches is listed here, and
/// `tests::every_further_route_costs_what_the_tool_costs` holds each of them to exactly the
/// price of the tool: a view that reached a dearer route would be a way around its scope, and
/// one that reached a cheaper route would be priced above its endpoint for no reason. The
/// coverage table reads this list too, so a capability reached only through a view counts.
///
/// Compiled with the tests only, like the coverage table: it decides nothing at run time — the
/// price is the tool's — it holds the tool to the routes it reaches.
#[cfg(test)]
#[rustfmt::skip]
pub(super) const TOOL_ALSO_REACHES: &[ToolPolicy] = &[
    tool("extract_packages", "/api/v1/packages/{id}/extract/force", Method::POST),
    tool("get_candidate_details", "/api/v1/collector/candidates/{id}/listing", Method::GET),
    tool("get_candidate_details", "/api/v1/collector/candidates/{id}/torrent", Method::GET),
    tool("get_nzb_import", "/api/v1/nzb/imports/{id}/postprocess", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent/peers", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent/pieces", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent/seeding", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent/stats", Method::GET),
    tool("get_torrent_details", "/api/v1/downloads/{id}/torrent/trackers", Method::GET),
    tool("get_torrent_engine", "/api/v1/torrents/network/status", Method::GET),
    tool("list_managed_tools", "/api/v1/system/media", Method::GET),
    tool("list_postprocess_options", "/api/v1/postprocess/plugin-steps", Method::GET),
    tool("list_postprocess_options", "/api/v1/postprocess/upload-destinations", Method::GET),
    tool("manage_tool", "/api/v1/system/tools/{name}/activate", Method::POST),
    tool("manage_tool", "/api/v1/system/tools/{name}/rollback", Method::POST),
    tool("preview_candidate_media", "/api/v1/collector/candidates/{id}/media/output-preview", Method::POST),
    tool("set_candidate_mirror", "/api/v1/collector/candidates/{id}/mirror", Method::DELETE),
    tool("set_candidate_mirror", "/api/v1/collector/candidates/{id}/mirror/dissolve", Method::POST),
    tool("set_candidate_plan", "/api/v1/collector/candidates/{id}/listing/plan", Method::PUT),
    tool("set_candidate_plan", "/api/v1/collector/candidates/{id}/torrent/plan", Method::PUT),
    tool("set_subscription_enabled", "/api/v1/subscriptions/{id}/disable", Method::POST),
    tool("set_category_seeding", "/api/v1/categories/{id}/seeding", Method::DELETE),
    tool("set_torrent_seeding", "/api/v1/downloads/{id}/torrent/seeding", Method::DELETE),
    tool("update_torrent_trackers", "/api/v1/downloads/{id}/torrent/trackers/reannounce", Method::POST),
    tool("update_torrent_trackers", "/api/v1/downloads/{id}/torrent/trackers/scrape", Method::POST),
];
