//! MCP tools for collision policies and prompts, duplicates, dedupe links and the storage
//! history (RD-150-01, RD-150-02).
//!
//! Everything the interface does with them, and nothing more: each tool calls the REST handler
//! with the arguments it was given, so the checks, the audit records and the codes are the
//! interface's own. Linking a duplicate replaces a file by a link to an identical one on this
//! machine, after both were hashed; it hands out no secret and changes nothing elsewhere, so it
//! is offered like the rest.

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::IdParams,
    params_storage::{
        CollisionDecisionParams, CollisionPolicyParams, DedupeParams, DuplicateLookupParams,
        StorageOperationsParams, decision, policy,
    },
};
use crate::{collision_handlers, duplicates, storage_handlers};

#[tool_router(router = collisions_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the collision policies: the global one (storage_collision_policy in the settings) and every category and package that has one of its own. A collision follows the package's policy, else its category's, else the global one."
    )]
    pub async fn list_collision_policies(&self) -> McpToolResult {
        respond(
            collision_handlers::list_collision_policies(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Set what happens when a file of this category meets a name that is taken: rename, skip, overwrite (audited, never over a file in use), compare (identical content is adopted) or ask (the download waits for a decision). Without `policy` the category inherits the global one again."
    )]
    pub async fn set_category_collision_policy(
        &self,
        Parameters(params): Parameters<CollisionPolicyParams>,
    ) -> McpToolResult {
        let result = async {
            let request = collision_handlers::SetCollisionPolicyRequest {
                policy: policy(params.policy.as_deref())?,
            };
            collision_handlers::set_category_collision_policy(
                State(self.state.clone()),
                AxumPath(params.id),
                Json(request),
            )
            .await
            .map(|Json(answer)| answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read one package's collision policy: its own, its category's, the global one, and which of them decides."
    )]
    pub async fn get_package_collision_policy(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            collision_handlers::get_package_collision_policy(
                State(self.state.clone()),
                AxumPath(params.id),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Set one package's own collision policy (rename, skip, overwrite, compare or ask); without `policy` the package inherits again. Answers the package's levels and the one that decides."
    )]
    pub async fn set_package_collision_policy(
        &self,
        Parameters(params): Parameters<CollisionPolicyParams>,
    ) -> McpToolResult {
        let result = async {
            let request = collision_handlers::SetCollisionPolicyRequest {
                policy: policy(params.policy.as_deref())?,
            };
            collision_handlers::set_package_collision_policy(
                State(self.state.clone()),
                AxumPath(params.id),
                Json(request),
            )
            .await
            .map(|Json(answer)| answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the downloads waiting for a collision decision (policy `ask`): which name is taken, how large the file there is, and whether the question came before the transfer or with the finished file waiting."
    )]
    pub async fn list_collision_prompts(&self) -> McpToolResult {
        respond(
            collision_handlers::list_collision_prompts(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Answer a collision prompt with rename, skip or overwrite; the download continues and carries the answer out. An overwrite is recorded in the audit log and is asked again if the file is in use by then."
    )]
    pub async fn decide_collision(
        &self,
        Parameters(params): Parameters<CollisionDecisionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = collision_handlers::CollisionDecisionRequest {
                decision: decision(&params.decision)?,
            };
            collision_handlers::decide_collision(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                AxumPath(params.id),
                Json(request),
            )
            .await
            .map(|Json(answer)| answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Explain a download's duplicates, in two separate lists: the same source queued or in the LinkGrabber (same address, magnet hash, NZB or hoster file), and the same content already on disk (same SHA-256, from the content index or a checksum the source stated)."
    )]
    pub async fn get_download_duplicates(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            duplicates::download_duplicates(State(self.state.clone()), AxumPath(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Look up addresses before they are queued: for each, its normalised source identity, the queue downloads of the same source and - while the setting duplicates_include_history is on (default off) - the download-history entries of the same source whose package has left the queue (history_id, name, outcome, finished_at)."
    )]
    pub async fn lookup_duplicates(
        &self,
        Parameters(params): Parameters<DuplicateLookupParams>,
    ) -> McpToolResult {
        respond(
            duplicates::lookup_duplicates(
                State(self.state.clone()),
                Json(duplicates::DuplicateLookupRequest { urls: params.urls }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Replace a finished download's file by a hard link to an identical finished file (ids from get_download_duplicates). Both are hashed first; refused across file systems, for files in use, and for files that differ. Recorded in the storage history and the audit log."
    )]
    pub async fn dedupe_download(
        &self,
        Parameters(params): Parameters<DedupeParams>,
    ) -> McpToolResult {
        let result = async {
            let request = storage_handlers::DedupeRequest {
                original_download_id: parse_id(&params.original_download_id)?,
                mode: storage_handlers::DedupeMode::Hardlink,
            };
            storage_handlers::dedupe_download(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                AxumPath(params.id),
                Json(request),
            )
            .await
            .map(|Json(answer)| answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read what each transfer kind can reuse of data on disk: resume a partial file, re-check it, adopt a finished file, verify the payload, and whether the collision policy applies to it."
    )]
    pub async fn get_storage_reuse(&self) -> McpToolResult {
        let Json(answer) = storage_handlers::reuse_capabilities(State(self.state.clone())).await;
        super::error::json_result(&answer)
    }

    #[tool(
        description = "Probe each storage root for hard-link support (and reflinks, which this build does not create)."
    )]
    pub async fn get_link_support(&self) -> McpToolResult {
        respond(
            storage_handlers::link_support(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the storage history, newest first: verified moves between folders and dedupe links, with the verified SHA-256 or the failure code."
    )]
    pub async fn list_storage_operations(
        &self,
        Parameters(params): Parameters<StorageOperationsParams>,
    ) -> McpToolResult {
        respond(
            storage_handlers::list_storage_operations(
                State(self.state.clone()),
                Query(storage_handlers::StorageOperationsQuery {
                    limit: params.limit,
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Check the content index against the disk now: files that vanished are marked missing, files that came back are found again, and finished downloads without an entry are added."
    )]
    pub async fn check_content_index(&self) -> McpToolResult {
        respond(
            storage_handlers::check_content_index(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }
}
