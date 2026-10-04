//! MCP (Model Context Protocol) server exposing rDownloader to AI assistants.
//!
//! Mounted at `/mcp` as a streamable-HTTP service behind the `api:*` bearer
//! token middleware. Tools delegate to the same code the REST handlers use — an
//! extracted `_inner` function where one exists, otherwise the handler itself,
//! called with its extractors constructed by hand — so validation, behaviour and
//! error codes stay identical and no rule is written twice.

mod error;
mod mask;
mod params;
mod params_config;
mod params_delivery;
mod params_handling;
mod params_insight;
mod params_remaining;
mod params_storage;
mod policy;
mod tools_backup;
mod tools_candidates;
mod tools_collector;
mod tools_collisions;
mod tools_config;
mod tools_containers;
mod tools_credentials;
mod tools_downloads;
mod tools_editors;
mod tools_grabber;
mod tools_indexers;
mod tools_insight;
mod tools_intake;
mod tools_notify;
mod tools_operations;
mod tools_pause;
mod tools_queue;
mod tools_remote;
mod tools_routing;
mod tools_site_rules;
mod tools_storage;
mod tools_stream_schedules;
mod tools_subscription_review;
mod tools_system;
mod tools_torrent;

use std::sync::Arc;

use axum::http::Method;
use rd_core::Scope;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool_handler,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_access::{audit_dto, audit_handlers};
use rd_api_admin::{
    about_page, automation_handlers, backup_destination_handlers, backup_handlers, config_handlers,
    data_reset_handlers, diagnostics_dto, diagnostics_handlers, notify_handlers, plugin_bundled,
    plugin_handlers, plugin_repository_handlers, plugin_update_policy, settings_handlers,
    stats_handlers, tools_handlers, update_handlers,
};
use rd_api_core::{
    ApiError, AppState, audit, auth, client, container_upload, dto, error_codes, hosters,
    postprocess_handlers, scope_policy, settings_store, storage_capacity,
};
use rd_api_intake::{
    candidate_handlers, collector_enqueue, collector_handlers, container_handlers,
    indexer_handlers, indexer_search, nzb_handlers, regex_tester, remote_listing_handlers,
    site_rules_dto, site_rules_handlers, stream_handlers, stream_schedule_handlers,
    subscription_handlers,
};
use rd_api_queue::{
    bandwidth_handlers, bandwidth_manual_handlers, collision_handlers, download_handlers,
    download_sources, duplicates, media_handlers, metrics, nzb_remote_job_handlers, package_clear,
    package_handlers, power_handlers, queue_pause_handlers, reconnect_handlers,
    remote_job_handlers, storage_handlers, torrent_control, torrent_handlers, torrent_trackers,
    usenet_handlers,
};

// Public for the coverage table in `rd-api`, which holds them against the assembled document.
pub use policy::{TOOL_ALSO_REACHES, TOOL_POLICY, ToolPolicy};

const INSTRUCTIONS: &str = "rDownloader download manager. Two ways to add downloads: \
(1) direct HTTP(S)/magnet URLs via add_downloads; \
(2) hoster/one-click links via collect_links (LinkGrabber analysis + online check), \
then check_links to see availability and enqueue_collector to start the downloads. \
A container file (.dlc, .torrent, .nzb, ...) is handed in as base64 with import_container, \
import_torrent or import_nzb. \
Progress is polled: call get_status_summary for the queue overview or list_downloads \
for per-file states. Settings changes via update_settings apply live. \
All byte values in settings are plain integers or JSON strings of integers. \
The configuration is writable too: categories, routing rules, storage roots, watched \
folders, provider accounts, proxies, NNTP servers, notification destinations and rules, \
subscriptions, livestream channels, automations and plugins each have create/update/delete \
tools. Jobs that run at a provider rather than here are listed, started, answered and \
forgotten with the remote_job tools; deleting one at the provider is not available here. \
To find out what happened: get_transfer_stats for volume over time, list_log_records and \
list_audit_records for the service log and who did what, both with the same filters the \
views offer. list_site_rules shows the release-page rules and which are active; \
create_site_rule, update_site_rule, test_site_rule and delete_site_rule write them. \
Everything the LinkGrabber screen does is here too: list_candidates names each link, and \
the candidate tools rename, move, reorder, enqueue, pick media variants, plan torrents and \
directory listings, and pin mirrors; list_nzb_imports and the nzb_import tools review and \
queue an NZB. search_indexers searches the Newznab indexers defined in the web UI \
(list_indexers) and grab_indexer_results puts chosen hits into the LinkGrabber as NZB imports. \
The queue is ordered with reorder_downloads and reorder_packages, renamed \
with rename_download, update_package and rename_package_folder, tidied with \
clear_finished_packages and unpacked with extract_packages. pause_queue pauses the whole \
queue for a while and resumes it by itself (resume_queue ends it early); \
switch_bandwidth_profile puts one of list_bandwidth_profiles in front of the schedule \
until its next change, a time or return_to_bandwidth_schedule. get_torrent_details, the \
seeding and tracker tools, list_postprocess_options, list_managed_tools and manage_tool, \
and get_storage_capacity cover the rest; get_about says which build is running, get_update_status and check_for_updates whether a newer one is out. The histories and catalogues beside the editors are \
here too: automation runs, versions, vocabulary and dry run, notification deliveries, the \
subscription review list and its polls, recording schedules and record-now, plugin runs, \
power and reconnect status, metrics and the diagnostic bundle's preview. What happens when a \
finished file meets a taken name is a collision policy (list_collision_policies and the set \
tools); downloads waiting for an answer are list_collision_prompts and decide_collision. \
get_download_duplicates explains source and content duplicates apart, dedupe_download links an \
identical file, and list_storage_operations shows verified moves and links; \
clear_storage_operations empties that history and clear_content_index the content index, both \
only with confirmed. Ids always come \
from a list tool first. \
Passwords, API keys and cookies are never accepted or returned by any tool; a row is \
created here and its credential is entered in the web UI.";

/// One MCP session facade over the shared application state.
#[derive(Clone)]
pub struct RdMcpServer {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl RdMcpServer {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::router(),
        }
    }

    /// Every tool this server offers, in one place so the tests build the same set.
    fn router() -> ToolRouter<Self> {
        Self::downloads_router()
            + Self::collector_router()
            + Self::containers_router()
            + Self::config_router()
            + Self::routing_router()
            + Self::storage_router()
            + Self::credentials_router()
            + Self::notify_router()
            + Self::intake_router()
            + Self::remote_router()
            + Self::insight_router()
            + Self::candidates_router()
            + Self::grabber_router()
            + Self::queue_router()
            + Self::torrent_router()
            + Self::system_router()
            + Self::site_rules_router()
            + Self::operations_router()
            + Self::editors_router()
            + Self::subscription_review_router()
            + Self::indexers_router()
            + Self::stream_schedules_router()
            + Self::collisions_router()
            + Self::backup_router()
            + Self::pause_router()
    }
}

tokio::task_local! {
    /// The scopes of the caller whose tool call is being served.
    ///
    /// Scoped around the call for the same reason as `crate::audit::CURRENT`: the credential is
    /// visible in `RdMcpServer::call_tool_unmasked` and not in the tool method. A tool whose
    /// *arguments* decide what it costs — `update_settings`, whose privileged fields need
    /// `api:admin` on top of the tool's `api:config` — reads it through [`granted_now`].
    static GRANTED: Vec<Scope>;
}

/// The scopes of the current tool call's caller, or none outside a call, which fails closed.
pub(crate) fn granted_now() -> Vec<Scope> {
    GRANTED.try_with(Clone::clone).unwrap_or_default()
}

/// The stable code a refused tool call carries, mirroring the REST refusal.
const SCOPE_INSUFFICIENT: &str = "auth.scope_insufficient";

/// The scope a tool costs, or `None` if the table does not know it.
fn tool_scope(name: &str) -> Option<Scope> {
    let entry = TOOL_POLICY.iter().find(|entry| entry.tool == name)?;
    match scope_policy::requirement(entry.path, &entry.method)? {
        // A tool reaching a public route would be a table mistake, not a free tool: the MCP
        // endpoint is already behind a token, so there is nothing to make public here.
        scope_policy::Requirement::Public => None,
        scope_policy::Requirement::Scope(scope) => Some(scope),
    }
}

/// The extra scope an argument makes this call cost.
///
/// `list_configuration` is one tool over six routes that are not priced alike — categories are
/// configuration, accounts are credentials, plugins are administration — and `Admin` does not
/// confer `Secrets`, so no single entry can cover it. The section therefore carries its own
/// price on top of the tool's.
fn argument_scope(
    name: &str,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<Scope> {
    if name != "list_configuration" {
        return None;
    }
    let section = arguments?.get("section")?.as_str()?;
    section_scope(section)
}

/// The route each `list_configuration` section reads, which is what prices it.
///
/// A table rather than a `match` arm because the `coverage` table reads it too: without it the
/// provider table would count as having no tool while `list_configuration` plainly lists it.
pub const CONFIG_SECTION_ROUTES: &[(&str, &str)] = &[
    ("accounts", "/api/v1/accounts"),
    ("categories", "/api/v1/categories"),
    ("plugins", "/api/v1/plugins"),
    ("providers", "/api/v1/providers"),
    ("proxy_profiles", "/api/v1/proxy-profiles"),
    ("storage_roots", "/api/v1/storage-roots"),
];

/// What one `list_configuration` section costs, mirroring the route that reads it.
fn section_scope(section: &str) -> Option<Scope> {
    let &(_, path) = CONFIG_SECTION_ROUTES
        .iter()
        .find(|(name, _)| *name == section)?;
    match scope_policy::requirement(path, &Method::GET)? {
        scope_policy::Requirement::Public => None,
        scope_policy::Requirement::Scope(scope) => Some(scope),
    }
}

/// The refusal a missing scope produces, carrying the same stable code the REST layer uses.
fn refusal(required: Scope, tool: &str) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_request(
        format!(
            "this token does not hold the {} permission that {tool} requires",
            required.as_str()
        ),
        Some(serde_json::json!({
            "code": SCOPE_INSUFFICIENT,
            "scope": required.as_str(),
            "tool": tool,
        })),
    )
}

impl RdMcpServer {
    /// The scopes the caller of this request carries, and who is making the call with which
    /// trace -- resolved once per tool call (audit 1.9.1, API-14).
    ///
    /// rmcp injects the originating `http::request::Parts` into the request context, which is
    /// the only place the credential is visible: the session factory that builds this server
    /// never sees a request, so the scopes cannot be resolved once per session. The transport
    /// gate (`auth::require_api_token`) already resolved them for this very request and left
    /// them in the parts' extensions; they are read from there, and looked up only for a
    /// request that did not pass the gate, which the router does not let happen.
    async fn caller(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> (Vec<Scope>, crate::audit::AuditContext) {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            // No HTTP parts means this is not the streamable-HTTP transport the service is
            // mounted on. Nothing to authorise against, so nothing is granted.
            return (
                Vec::new(),
                crate::audit::AuditContext {
                    actor: crate::audit::Actor::system(),
                    trace: None,
                },
            );
        };
        let trace = parts.extensions.get::<rd_core::TraceContext>().copied();
        let resolved = parts
            .extensions
            .get::<crate::auth::Granted>()
            .zip(parts.extensions.get::<crate::audit::Actor>());
        let (scopes, actor) = match resolved {
            Some((granted, actor)) => (granted.scopes().to_vec(), actor.clone()),
            None => {
                let from_this_machine =
                    crate::client::from_this_machine(&parts.extensions, &parts.headers);
                crate::auth::credential(&self.state, &parts.headers, from_this_machine).await
            }
        };
        (scopes, crate::audit::AuditContext { actor, trace })
    }

    /// The permission check and the call itself, before the mask.
    async fn call_tool_unmasked(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        let Some(required) = tool_scope(&request.name) else {
            // Cannot happen while the test below passes. Fails closed anyway.
            return Err(rmcp::ErrorData::invalid_request(
                format!("no permission is defined for the tool {}", request.name),
                Some(serde_json::json!({
                    "code": "scope.route_unclassified",
                    "tool": request.name.as_ref(),
                })),
            ));
        };
        let (granted, audit) = self.caller(&context).await;
        if !granted.contains(&required) {
            return Err(refusal(required, &request.name));
        }
        if let Some(extra) = argument_scope(&request.name, request.arguments.as_ref())
            && !granted.contains(&extra)
        {
            return Err(refusal(extra, &request.name));
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        // Scoped rather than passed: the generated tool methods take only their parameters,
        // and the credential is visible here and nowhere below (RD-110-03).
        GRANTED
            .scope(
                granted,
                crate::audit::with_context(audit, self.tool_router.call(tcc)),
            )
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RdMcpServer {
    /// One check and one mask for every tool.
    ///
    /// Placed here rather than inside each tool on purpose: sixty copies of the same three
    /// lines is sixty chances to omit one, and an omitted check is invisible. The macro
    /// generates this method only when the impl does not define it, so providing it replaces
    /// the generated passthrough and keeps `list_tools` as it was.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        // Every answer, refusals included, leaves through the one mask (RD-120-57): an address
        // loses its credentials here, so no tool -- present or future -- can forget to.
        mask::mask_response(self.call_tool_unmasked(request, context).await)
    }

    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS);
        let mut implementation = Implementation::default();
        implementation.name = "rdownloader".to_owned();
        implementation.title = Some("rDownloader".to_owned());
        implementation.version = env!("CARGO_PKG_VERSION").to_owned();
        info.server_info = implementation;
        info
    }
}

/// Builds the tower service handling POST/GET/DELETE on the `/mcp` path.
///
/// Host validation is disabled: the route already requires an `api:*` bearer
/// token (or session), which neutralizes DNS-rebinding concerns, and the
/// service must stay reachable when the server is bound to a LAN address.
///
/// The sessions and their event streams end when the service stops: the graceful stop waits for
/// every open response, and an MCP client's `GET` stream never ends by itself (RD-180-02). A
/// child of the stop token, so nothing the transport cancels reaches the service's own stop.
pub fn service(state: AppState) -> StreamableHttpService<RdMcpServer, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_cancellation_token(state.shutdown.child_token());
    StreamableHttpService::new(
        move || Ok(RdMcpServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[cfg(test)]
mod tests;
