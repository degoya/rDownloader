//! The tools that read and need no id of their own, shared by the read sweep in `mcp.rs` and
//! RD-120-57's indexer-key test; a module of its own so the suite stays under its length limit.

/// Every tool that reads and needs no id of its own. The six `list_configuration` sections
/// are named individually because each reads a different store, and `accounts`,
/// `proxy_profiles` and `plugins` are the three that touch what was just seeded.
/// Shared with RD-120-57's indexer-key test, which calls them with a key in the LinkGrabber.
pub(super) fn id_free_reads() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("list_downloads", serde_json::json!({})),
        ("get_status_summary", serde_json::json!({})),
        ("list_packages", serde_json::json!({})),
        ("list_collector", serde_json::json!({})),
        ("get_settings", serde_json::json!({})),
        ("get_about", serde_json::json!({})),
        (
            "list_configuration",
            serde_json::json!({ "section": "accounts" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "proxy_profiles" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "categories" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "storage_roots" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "providers" }),
        ),
        (
            "list_configuration",
            serde_json::json!({ "section": "plugins" }),
        ),
        ("list_category_rules", serde_json::json!({})),
        ("list_hotfolders", serde_json::json!({})),
        ("list_automations", serde_json::json!({})),
        ("list_notification_rules", serde_json::json!({})),
        ("list_notification_targets", serde_json::json!({})),
        ("list_web_push_subscriptions", serde_json::json!({})),
        ("list_subscriptions", serde_json::json!({})),
        ("list_stream_channels", serde_json::json!({})),
        ("list_usenet_servers", serde_json::json!({})),
        ("list_indexers", serde_json::json!({})),
        ("list_remote_jobs", serde_json::json!({})),
        ("list_site_rules", serde_json::json!({})),
        ("get_transfer_stats", serde_json::json!({})),
        ("get_usenet_server_traffic", serde_json::json!({})),
        ("list_log_records", serde_json::json!({ "limit": 500 })),
        ("list_audit_records", serde_json::json!({ "limit": 500 })),
        // RD-120-32's reads that need no id. The ones that do are searched in `everything`.
        ("list_candidates", serde_json::json!({})),
        ("get_mirror_preference", serde_json::json!({})),
        ("list_nzb_imports", serde_json::json!({})),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "scripts" }),
        ),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "plugin_steps" }),
        ),
        (
            "list_postprocess_options",
            serde_json::json!({ "kind": "upload_destinations" }),
        ),
        ("list_postprocess_queue", serde_json::json!({})),
        ("list_managed_tools", serde_json::json!({ "view": "tools" })),
        ("list_managed_tools", serde_json::json!({ "view": "media" })),
        ("get_storage_capacity", serde_json::json!({})),
        (
            "get_torrent_engine",
            serde_json::json!({ "view": "capabilities" }),
        ),
        (
            "get_torrent_engine",
            serde_json::json!({ "view": "network_status" }),
        ),
        ("list_network_interfaces", serde_json::json!({})),
    ]
}
