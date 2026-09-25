//! MCP (Model Context Protocol) server exposing rDownloader to AI assistants.
//!
//! Mounted at `/mcp` as a streamable-HTTP service behind the `api:*` bearer
//! token middleware. Tools delegate to the same code the REST handlers use — an
//! extracted `_inner` function where one exists, otherwise the handler itself,
//! called with its extractors constructed by hand — so validation, behaviour and
//! error codes stay identical and no rule is written twice.

// Verification only, so it is compiled with the tests: the table decides nothing at run time,
// it holds the toolbox against the REST surface. `cargo nextest run -p rd-api --lib` is where
// an undecided route fails.
#[cfg(test)]
mod coverage;
mod error;
mod mask;
mod params;
mod params_config;
mod params_delivery;
mod params_handling;
mod params_insight;
mod params_remaining;
mod policy;
mod tools_candidates;
mod tools_collector;
mod tools_config;
mod tools_containers;
mod tools_credentials;
mod tools_downloads;
mod tools_editors;
mod tools_grabber;
mod tools_insight;
mod tools_intake;
mod tools_notify;
mod tools_operations;
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
    model::{Implementation, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool_handler,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};

use crate::{AppState, scope_policy};
#[cfg(test)]
use policy::TOOL_ALSO_REACHES;
use policy::TOOL_POLICY;

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
queue an NZB. The queue is ordered with reorder_downloads and reorder_packages, renamed \
with rename_download, update_package and rename_package_folder, tidied with \
clear_finished_packages and unpacked with extract_packages. get_torrent_details, the \
seeding and tracker tools, list_postprocess_options, list_managed_tools and manage_tool, \
and get_storage_capacity cover the rest; get_about says which build is running. The histories and catalogues beside the editors are \
here too: automation runs, versions, vocabulary and dry run, notification deliveries, the \
subscription review list and its polls, recording schedules and record-now, plugin runs, \
power and reconnect status, metrics and the diagnostic bundle's preview. Ids always come \
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
            + Self::stream_schedules_router()
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
const CONFIG_SECTION_ROUTES: &[(&str, &str)] = &[
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
    /// The scopes the caller of this request carries.
    ///
    /// rmcp injects the originating `http::request::Parts` into the request context, which is
    /// the only place the credential is visible: the session factory that builds this server
    /// never sees a request, so the scopes cannot be resolved once per session.
    async fn granted(&self, context: &RequestContext<RoleServer>) -> Vec<Scope> {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            // No HTTP parts means this is not the streamable-HTTP transport the service is
            // mounted on. Nothing to authorise against, so nothing is granted.
            return Vec::new();
        };
        crate::auth::granted_scopes(&self.state, &parts.headers).await
    }

    /// Who is making this tool call, and which trace it belongs to.
    ///
    /// Same source as [`Self::granted`] and for the same reason: the session factory never
    /// sees a request, so both have to be resolved per call from the injected parts.
    async fn audit_context(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> crate::audit::AuditContext {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return crate::audit::AuditContext {
                actor: crate::audit::Actor::system(),
                trace: None,
            };
        };
        let (_scopes, actor) = crate::auth::credential(&self.state, &parts.headers).await;
        crate::audit::AuditContext {
            actor,
            trace: parts.extensions.get::<rd_core::TraceContext>().copied(),
        }
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
        let granted = self.granted(&context).await;
        if !granted.contains(&required) {
            return Err(refusal(required, &request.name));
        }
        if let Some(extra) = argument_scope(&request.name, request.arguments.as_ref())
            && !granted.contains(&extra)
        {
            return Err(refusal(extra, &request.name));
        }
        let audit = self.audit_context(&context).await;
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

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
pub(crate) fn service(state: AppState) -> StreamableHttpService<RdMcpServer, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    StreamableHttpService::new(
        move || Ok(RdMcpServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[cfg(test)]
mod tests;
