//! The tool table held against the router and the route prices (RD-101-16, RD-120-32).
//!
//! Moved out of `mod.rs` when RD-120-55 took that file past 500 lines.

use super::{
    RdMcpServer, Scope, TOOL_ALSO_REACHES, TOOL_POLICY, scope_policy, section_scope, tool_scope,
};

/// The table and the router describe the same tools, in both directions.
///
/// A tool added without an entry would reach the `None` branch in `call_tool` and be
/// refused — safe, but a build failure is a better place to find out than a bug report.
/// An entry for a tool that no longer exists is the opposite problem: a permission
/// decision that reads as coverage of something that is gone.
#[test]
fn the_tool_policy_and_the_router_describe_the_same_tools() {
    let mut registered: Vec<String> = RdMcpServer::router()
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    registered.sort();

    let mut classified: Vec<String> = TOOL_POLICY
        .iter()
        .map(|entry| entry.tool.to_owned())
        .collect();
    classified.sort();

    assert_eq!(
        registered, classified,
        "the MCP tool policy and the tool router disagree"
    );
}

/// Kept sorted so a diff shows the addition rather than a reshuffle.
#[test]
fn the_tool_policy_is_sorted() {
    let names: Vec<&str> = TOOL_POLICY.iter().map(|entry| entry.tool).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

/// Every tool names a route the scope table actually prices.
///
/// This is what makes "a tool is never cheaper than the endpoint underneath it" a fact
/// rather than a promise: the price is *read* from `scope_policy`, so the only way to get
/// it wrong is to name the wrong route, and a route that does not exist fails here.
#[test]
fn every_tool_is_priced_by_a_route_that_exists() {
    for entry in TOOL_POLICY {
        let requirement = scope_policy::requirement(entry.path, &entry.method);
        assert!(
            matches!(requirement, Some(scope_policy::Requirement::Scope(_))),
            "{} names {} {}, which the route policy does not price",
            entry.tool,
            entry.method,
            entry.path
        );
        assert!(tool_scope(entry.tool).is_some(), "{}", entry.tool);
    }
}

/// A tool that reaches more than one route costs the same for every one of them.
///
/// The views and actions RD-120-32 folded into one tool each are priced by the tool's own
/// entry, so a further route that cost more would be reachable below its price. This reads
/// every further route's price from `scope_policy` and holds it to the tool's.
#[test]
fn every_further_route_costs_what_the_tool_costs() {
    for entry in TOOL_ALSO_REACHES {
        let tool = tool_scope(entry.tool)
            .unwrap_or_else(|| panic!("{} reaches further routes but has no price", entry.tool));
        let route = match scope_policy::requirement(entry.path, &entry.method) {
            Some(scope_policy::Requirement::Scope(scope)) => scope,
            other => panic!(
                "{} reaches {} {}, which is priced {other:?}",
                entry.tool, entry.method, entry.path
            ),
        };
        assert_eq!(
            route, tool,
            "{} costs {tool:?} but also reaches {} {}, which costs {route:?}",
            entry.tool, entry.method, entry.path
        );
        assert!(
            !TOOL_POLICY
                .iter()
                .any(|primary| primary.path == entry.path && primary.method == entry.method),
            "{} {} is already some tool's own route",
            entry.method,
            entry.path
        );
    }
}

/// The tools that reach credentials, administration or the settings document are dear.
///
/// Spot-checked by name because these are the entries whose misclassification would
/// matter most: an assistant given the queue must not thereby be able to read every
/// stored account, uninstall a plugin or rewrite the settings document.
#[test]
fn the_costly_tools_are_priced_accordingly() {
    assert_eq!(tool_scope("delete_account"), Some(Scope::Secrets));
    assert_eq!(tool_scope("update_usenet_server"), Some(Scope::Secrets));
    assert_eq!(tool_scope("delete_proxy_profile"), Some(Scope::Secrets));
    assert_eq!(tool_scope("uninstall_plugin_version"), Some(Scope::Admin));
    assert_eq!(tool_scope("update_settings"), Some(Scope::Config));
    assert_eq!(tool_scope("delete_category"), Some(Scope::Config));
    assert_eq!(tool_scope("delete_packages"), Some(Scope::Queue));
    assert_eq!(tool_scope("list_downloads"), Some(Scope::Read));
    assert_eq!(tool_scope("import_container"), Some(Scope::Intake));
    assert_eq!(tool_scope("import_nzb"), Some(Scope::Intake));
    assert_eq!(tool_scope("import_torrent"), Some(Scope::Intake));
    // RD-120-32: each priced as its route, spot-checked across the four prices it spans.
    assert_eq!(tool_scope("get_torrent_details"), Some(Scope::Read));
    assert_eq!(tool_scope("get_storage_capacity"), Some(Scope::Read));
    assert_eq!(tool_scope("get_about"), Some(Scope::Read));
    assert_eq!(tool_scope("list_candidates"), Some(Scope::Queue));
    assert_eq!(tool_scope("clear_finished_packages"), Some(Scope::Queue));
    assert_eq!(tool_scope("enqueue_nzb_import"), Some(Scope::Queue));
    assert_eq!(tool_scope("manage_tool"), Some(Scope::Config));
    assert_eq!(tool_scope("create_site_rule"), Some(Scope::Config));
    assert_eq!(tool_scope("set_category_seeding"), Some(Scope::Config));
    assert_eq!(tool_scope("list_network_interfaces"), Some(Scope::Config));
    // RD-120-55: the thirteen span every price the ladder has, metrics included.
    assert_eq!(tool_scope("get_metrics"), Some(Scope::Metrics));
    assert_eq!(tool_scope("list_account_hosters"), Some(Scope::Secrets));
    assert_eq!(tool_scope("preview_diagnostic_bundle"), Some(Scope::Admin));
    assert_eq!(tool_scope("cancel_power_action"), Some(Scope::Admin));
    assert_eq!(tool_scope("record_stream_now"), Some(Scope::Intake));
    assert_eq!(tool_scope("poll_subscription"), Some(Scope::Queue));
    assert_eq!(tool_scope("create_stream_schedule"), Some(Scope::Config));
    assert_eq!(tool_scope("get_plugin_messages"), Some(Scope::Read));
    assert_eq!(tool_scope("nothing_like_this"), None);
}

/// `list_configuration` costs what the section it is asked for costs.
///
/// One tool over six routes that are not priced alike. Reading the categories is
/// configuration; reading the accounts discloses where credentials exist; reading the
/// plugin inventory is administration. A single price would have to be the dearest one,
/// which would put the category list behind `api:admin`, or the cheapest, which is the
/// bug this pins down.
#[test]
fn each_configuration_section_costs_what_its_route_costs() {
    assert_eq!(section_scope("categories"), Some(Scope::Config));
    assert_eq!(section_scope("storage_roots"), Some(Scope::Config));
    assert_eq!(section_scope("providers"), Some(Scope::Config));
    assert_eq!(section_scope("accounts"), Some(Scope::Secrets));
    assert_eq!(section_scope("proxy_profiles"), Some(Scope::Secrets));
    assert_eq!(section_scope("plugins"), Some(Scope::Admin));
    assert_eq!(section_scope("nothing_like_this"), None);
}

/// No tool takes a credential.
///
/// The vault is filled in the web UI. A tool parameter called `password` or `api_key`
/// would put the value into a model's context, a transcript and whatever the client logs,
/// and it would do so silently — nothing about a tool schema announces that one of its
/// fields is a secret. This walks every published input schema instead of trusting review.
#[test]
fn no_tool_accepts_a_credential() {
    /// The one credential-shaped parameter that is not a stored credential: an archive
    /// password travels with the links it unlocks and never reaches the vault.
    const ALLOWED: &[(&str, &str)] = &[("collect_links", "password")];

    fn walk(tool: &str, schema: &serde_json::Value, found: &mut Vec<String>) {
        match schema {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Object(properties)) = map.get("properties") {
                    for name in properties.keys() {
                        if super::error::CREDENTIAL_FIELDS.contains(&name.as_str())
                            && !ALLOWED.contains(&(tool, name.as_str()))
                        {
                            found.push(name.clone());
                        }
                    }
                }
                for value in map.values() {
                    walk(tool, value, found);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    walk(tool, value, found);
                }
            }
            _ => {}
        }
    }

    for published in RdMcpServer::router().list_all() {
        let schema = serde_json::Value::Object((*published.input_schema).clone());
        let mut found = Vec::new();
        walk(&published.name, &schema, &mut found);
        assert!(
            found.is_empty(),
            "the tool {} takes credential parameters: {found:?}",
            published.name
        );
    }
}
