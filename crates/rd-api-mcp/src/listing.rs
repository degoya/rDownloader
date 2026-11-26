//! What `tools/list` says about the tools, and to whom (RD-1480-06).
//!
//! **Annotations.** Each tool's hints follow from the methods of the routes it reaches
//! ([`TOOL_POLICY`] and [`TOOL_ALSO_REACHES`]), so a tool cannot claim to be gentler than its
//! endpoint: `GET` alone is read-only, a `DELETE` destructive, `PUT`, `PATCH` and `DELETE`
//! idempotent. A `PUT` or `PATCH` is not called destructive: it replaces the value it names and
//! loses nothing beside it. Where the method says too little, the tool is named below with the
//! group it belongs to. The hints are for the client; what a call may do is decided by the
//! token's scopes and by the confirmation the clearing tools ask for.
//!
//! **The list.** A tool the token may not call is left out: its schema is context the model pays
//! for and can do nothing with. This is no permission check -- [`RdMcpServer::call_tool`] asks
//! for the scope of every call, listed or not.

use rd_core::Scope;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{Tool, ToolAnnotations},
};

use axum::http::Method;

use super::{RdMcpServer, TOOL_ALSO_REACHES, TOOL_POLICY, tool_scope};

/// Tools over a `POST` that change nothing here: the body is the question. A tool that fetches an
/// address of the call's choosing (`test_site_rule`, `test_malware_scanner`) is not among them --
/// a client that lets read-only tools through unasked would let the model reach any host. Sorted.
const READ_ONLY_POSTS: &[&str] = &[
    "dry_run_automations",
    "lookup_duplicates",
    "preview_candidate_media",
    "preview_category_sorting",
    "preview_package_name",
    "search_indexers",
    "test_category_regex",
];

/// Tools without a `DELETE` that remove or overwrite something the call does not replace: rows,
/// files on disk, histories, links. Sorted.
const DESTRUCTIVE_WITHOUT_DELETE: &[&str] = &[
    "apply_link_filters",
    "clean_up_data_directory",
    "clear_audit_records",
    "clear_content_index",
    "clear_download_history",
    "clear_finished_packages",
    "clear_log_records",
    "clear_notification_deliveries",
    "clear_remote_jobs",
    "clear_storage_operations",
    "clear_transfer_stats",
    "control_downloads",
    "decide_collision",
    "dedupe_download",
    "delete_packages",
    "discard_pending_notification_deliveries",
    "hand_over_collector_package",
    "regroup_collector",
    "remove_bundled_services",
    "reset_automation_versions",
    "reset_package",
];

/// Tools over a `DELETE` that only end a mark, a pause or an override; nothing is lost. Sorted.
const HARMLESS_DELETES: &[&str] = &[
    "clear_stop_mark",
    "resume_queue",
    "return_to_bandwidth_schedule",
    "set_candidate_mirror",
    "set_category_seeding",
    "set_torrent_seeding",
    "stop_log_debug",
];

/// The methods of every route `tool` reaches.
fn methods(tool: &str) -> impl Iterator<Item = &'static Method> {
    TOOL_POLICY
        .iter()
        .chain(TOOL_ALSO_REACHES)
        .filter(move |entry| entry.tool == tool)
        .map(|entry| &entry.method)
}

/// The hints of one tool, from its routes' methods and the groups above.
///
/// A read-only tool carries that hint alone; the other two say nothing about it.
fn annotations(tool: &str) -> ToolAnnotations {
    let mut reached = methods(tool).peekable();
    let only_gets = reached.peek().is_some() && reached.all(|method| *method == Method::GET);
    if only_gets || READ_ONLY_POSTS.contains(&tool) {
        return ToolAnnotations::new().read_only(true);
    }
    let destructive = DESTRUCTIVE_WITHOUT_DELETE.contains(&tool)
        || (methods(tool).any(|method| *method == Method::DELETE)
            && !HARMLESS_DELETES.contains(&tool));
    let idempotent = methods(tool).all(|method| *method != Method::POST);
    ToolAnnotations::new()
        .read_only(false)
        .destructive(destructive)
        .idempotent(idempotent)
}

/// Writes the hints onto every tool the router holds.
pub(crate) fn annotate<S>(router: &mut ToolRouter<S>) {
    for (name, route) in &mut router.map {
        route.attr.annotations = Some(annotations(name));
    }
}

/// The tools a caller holding `granted` may call, in the router's order.
pub(crate) fn offered(router: &ToolRouter<RdMcpServer>, granted: &[Scope]) -> Vec<Tool> {
    let mut tools = router.list_all();
    tools.retain(|tool| tool_scope(&tool.name).is_some_and(|scope| granted.contains(&scope)));
    tools
}

#[cfg(test)]
mod tests {
    use super::{
        DESTRUCTIVE_WITHOUT_DELETE, HARMLESS_DELETES, Method, READ_ONLY_POSTS, Scope, annotations,
        methods, offered,
    };
    use crate::{RdMcpServer, TOOL_POLICY};

    /// Each exception names a tool, says what its methods do not, and is sorted.
    #[test]
    fn every_exception_is_one() {
        let gets_only = |tool: &str| methods(tool).all(|method| *method == Method::GET);
        let deletes = |tool: &str| methods(tool).any(|method| *method == Method::DELETE);
        for list in [
            READ_ONLY_POSTS,
            DESTRUCTIVE_WITHOUT_DELETE,
            HARMLESS_DELETES,
        ] {
            let mut sorted = list.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted, list);
            for tool in list {
                assert!(methods(tool).next().is_some(), "{tool} is not a tool");
                assert!(!gets_only(tool), "{tool} is read-only by its methods");
            }
        }
        for tool in READ_ONLY_POSTS {
            assert!(!deletes(tool), "{tool} deletes");
        }
        for tool in DESTRUCTIVE_WITHOUT_DELETE {
            assert!(!deletes(tool), "{tool} is destructive by its methods");
            assert!(!READ_ONLY_POSTS.contains(tool), "{tool} is in two groups");
        }
        for tool in HARMLESS_DELETES {
            assert!(deletes(tool), "{tool} has no DELETE");
        }
    }

    /// The published list carries the hints, and a tool's name agrees with them: what is called
    /// `get_` or `list_` reads, what is called `delete_`, `remove_`, `uninstall_` or `clear_`
    /// removes -- but for the names that promise something else, named here.
    #[test]
    fn the_hints_agree_with_the_names() {
        // Fetches a page and keeps its entries for the choice.
        const LISTS_AND_KEEPS: &[&str] = &["list_page_entries"];
        for tool in RdMcpServer::router().list_all() {
            let hints = tool.annotations.clone().expect("every tool is annotated");
            assert_eq!(hints, annotations(&tool.name));
            let name: &str = &tool.name;
            let reads = name.starts_with("get_") || name.starts_with("list_");
            let removes = ["delete_", "remove_", "uninstall_", "clear_"]
                .iter()
                .any(|prefix| name.starts_with(prefix));
            if reads && !LISTS_AND_KEEPS.contains(&name) {
                assert_eq!(hints.read_only_hint, Some(true), "{name}");
            }
            if removes && !HARMLESS_DELETES.contains(&name) {
                assert_eq!(hints.read_only_hint, Some(false), "{name}");
                assert_eq!(hints.destructive_hint, Some(true), "{name}");
            }
        }
    }

    /// Spot checks over the four methods and the three groups.
    #[test]
    fn the_hints_follow_the_methods() {
        let read = annotations("get_settings");
        assert_eq!(read.read_only_hint, Some(true));
        assert_eq!(read.destructive_hint, None);

        let create = annotations("create_category");
        assert_eq!(create.read_only_hint, Some(false));
        assert_eq!(create.destructive_hint, Some(false));
        assert_eq!(create.idempotent_hint, Some(false));

        let update = annotations("update_settings");
        assert_eq!(update.destructive_hint, Some(false));
        assert_eq!(update.idempotent_hint, Some(true));

        let delete = annotations("delete_category");
        assert_eq!(delete.destructive_hint, Some(true));
        assert_eq!(delete.idempotent_hint, Some(true));

        assert_eq!(annotations("search_indexers").read_only_hint, Some(true));
        assert_eq!(annotations("delete_packages").destructive_hint, Some(true));
        assert_eq!(annotations("resume_queue").destructive_hint, Some(false));
        assert_eq!(annotations("test_site_rule").read_only_hint, Some(false));
        // Reached by a POST beside its PUT.
        let trackers = annotations("update_torrent_trackers");
        assert_eq!(trackers.idempotent_hint, Some(false));
    }

    /// The list holds exactly what the scopes price, whatever they are, and nothing for none.
    #[test]
    fn a_token_is_offered_the_tools_it_may_call() {
        let router = RdMcpServer::router();
        assert!(offered(&router, &[]).is_empty());
        assert_eq!(offered(&router, Scope::API).len(), TOOL_POLICY.len());

        let names = |granted: &[Scope]| -> Vec<String> {
            offered(&router, granted)
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        };
        let read = names(&[Scope::Read]);
        assert!(read.contains(&"list_downloads".to_owned()));
        assert!(!read.contains(&"delete_packages".to_owned()));
        assert!(!read.contains(&"update_settings".to_owned()));
        let queue = names(&[Scope::Read, Scope::Queue]);
        assert!(queue.contains(&"delete_packages".to_owned()));
        assert!(!queue.contains(&"delete_account".to_owned()));
    }
}
