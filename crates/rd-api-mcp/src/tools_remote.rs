//! MCP tools for jobs that run at a provider rather than on this machine (RD-120-29).
//!
//! Remote jobs were a whole view with no tools at all: `grep -rn remote_job crates/rd-api/src/mcp/`
//! found nothing, which is how RD-120-23 discovered that a container cannot be handed in over
//! MCP -- the path was not there to begin with. Four of the five routes are here. The fifth,
//! `discard`, deletes the job at the provider and refuses anything without an explicit
//! confirmation; a tool passing that confirmation would be the assistant confirming on the
//! person's behalf, so it stays out. `forget_remote_job` is the half that only touches this
//! installation's own list, and it says so in its own answer. `submit_nzb_import_remote_job`
//! hands an NZB import from the LinkGrabber to a provider the same way, and
//! `submit_package_remote_job` the NZB behind a queued package (RD-191-13), and
//! `hand_over_collector_package` every link of a LinkGrabber package (RD-1440-07), and
//! `hand_over_subscription_item` the NZB of a subscription's hit waiting for review (RD-1460-03).
//!
//! `clear_remote_jobs` clears the list, or the part a provider and state filter selects
//! (RD-1200-01), here only, behind the question every clearing tool asks first since RD-1190-21.
//! The REST route's `at_provider` variant stays web-only by the owner's line of 2026-09-23 --
//! no tool changes something irreversibly outside this machine -- which the owner confirmed for
//! this tool on 2026-10-08: the tool always sends `at_provider: false`.

use axum::extract::State;
use rd_api_core::extract::{Json, Path as AxumPath};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::IdParams,
    params_insight::{
        ClearRemoteJobsParams, HandOverCollectorPackageParams, HandOverSubscriptionItemParams,
        RemoteJobChoiceParams, SubmitNzbImportRemoteJobParams, SubmitPackageRemoteJobParams,
        SubmitRemoteJobParams,
    },
};
use crate::remote_job_handlers::{
    RemoteJobChoiceRequest, RemoteJobClearRequest, SubmitRemoteJobRequest,
};

#[tool_router(router = remote_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List every job running at a provider, newest first: which account it belongs to, its source (`source_kind` magnet, container or address; `source_name` the .torrent/.nzb file name, a magnet's dn or an address's last path segment, absent when there is none; `content_key` the info hash or digest), the LinkGrabber package its finished files went to (`package_id`), its state (submitting, preparing, awaiting_choice, working, ready, failed, discarded), and the entries a job in awaiting_choice is asking about. No credential is included."
    )]
    pub async fn list_remote_jobs(&self) -> McpToolResult {
        respond(
            crate::remote_job_handlers::list_remote_jobs(State(self.state.clone()))
                .await
                .map(|jobs| jobs.0),
        )
    }

    #[tool(
        description = "Hand a magnet address, a plain http(s) address or a .torrent/.nzb file (base64, at most 16 MiB) to one account's provider to fetch on its own side. Give exactly one of magnet, address and container. Answers with the job row, named by `file_name`, else a magnet's dn or an address's last path segment; `already_running` true means this account already had a job for this content and nothing was sent."
    )]
    pub async fn submit_remote_job(
        &self,
        Parameters(params): Parameters<SubmitRemoteJobParams>,
    ) -> McpToolResult {
        let result = async {
            let account = parse_id(&params.account_id)?;
            Ok(crate::remote_job_handlers::submit_remote_job(
                State(self.state.clone()),
                AxumPath(account),
                Json(SubmitRemoteJobRequest {
                    magnet: params.magnet,
                    address: params.address,
                    container: params.container,
                    file_name: params.file_name,
                }),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Hand an NZB import still waiting in the LinkGrabber to one account's provider, which fetches it from Usenet on its side; the finished files come back into the LinkGrabber as links. Only accounts whose provider takes NZB files are accepted (list_remote_job_providers with container nzb). The import stays in the LinkGrabber marked `handed_over`; `already_running` true means this account already had a job for this NZB and nothing was sent."
    )]
    pub async fn submit_nzb_import_remote_job(
        &self,
        Parameters(params): Parameters<SubmitNzbImportRemoteJobParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let account_id = parse_id(&params.account_id)?;
            Ok(
                crate::nzb_remote_job_handlers::submit_nzb_import_remote_job(
                    State(self.state.clone()),
                    AxumPath(id),
                    Json(crate::nzb_remote_job_handlers::NzbImportRemoteJobRequest { account_id }),
                )
                .await?
                .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Hand the NZB behind a package in the download list to one account's provider, in any state of the package (a failed one included); the provider fetches it from Usenet on its side and the finished files come back into the LinkGrabber as links. The package stays; its NZB import is marked `handed_over`. Refused with package.remote_job_no_nzb when the package did not come from an NZB. Only accounts whose provider takes NZB files are accepted (list_remote_job_providers with container nzb); `already_running` true means this account already had a job for this NZB and nothing was sent."
    )]
    pub async fn submit_package_remote_job(
        &self,
        Parameters(params): Parameters<SubmitPackageRemoteJobParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let account_id = parse_id(&params.account_id)?;
            Ok(crate::nzb_remote_job_handlers::submit_package_remote_job(
                State(self.state.clone()),
                AxumPath(id),
                Json(crate::nzb_remote_job_handlers::NzbImportRemoteJobRequest { account_id }),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Hand every link of a LinkGrabber package to one account's provider instead of the queue: a magnet, an http(s) address, an imported .torrent or the document behind an NZB link (fetched from the indexer here, its address and API key never sent; an unchecked link in an indexer's form or at a configured indexer is treated as one) each becomes one remote job, through the same duplicate guard as submit_remote_job; the provider fetches the content and the finished files come back into the LinkGrabber as links. Refused with package.not_found, remote_job.no_account or, when no installed plugin runs remote jobs on the account's provider, collector.remote_job_not_remote_account. Otherwise answers `handed_over` (each link with its job; `already_running`: the account already had it and nothing was sent) and `stayed` (each link that could not go, with its code: refused by the provider, not claimed, a source no provider takes, an address that carries a login or an access key (collector.remote_job_secret_address), an NZB the provider does not take or the indexer refused (nzb.remote_job_no_nzb, collector.nzb_*), hidden by a LinkFilter rule, a mirror that is not the chosen copy, being queued). The setting linkgrabber_hand_over_remove_links decides what becomes of the links handed over, never this call: on (default) they leave the LinkGrabber and a package left empty is deleted (`links_removed`, `package_removed`); off they stay marked `handed_over` and a second call sends nothing again. What stayed always stays."
    )]
    pub async fn hand_over_collector_package(
        &self,
        Parameters(params): Parameters<HandOverCollectorPackageParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let account_id = parse_id(&params.account_id)?;
            Ok(
                crate::collector_handlers::submit_collector_package_remote_job(
                    State(self.state.clone()),
                    AxumPath(id),
                    Json(
                        crate::collector_handlers::CollectorPackageRemoteJobRequest { account_id },
                    ),
                )
                .await?
                .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Hand a subscription's hit that is waiting for review (item id from list_subscription_items) to one account's provider instead of the LinkGrabber: its NZB is fetched from the indexer here and handed over as a .nzb file, never the indexer's address with its key, through the same checks and duplicate guard as submit_nzb_import_remote_job. Afterwards the hit is decided as after review_subscription_item with state=queued. Refused with subscription.item_not_found, subscription.remote_job_item_decided (the hit is no longer waiting), remote_job.no_account, nzb.remote_job_not_remote_account or nzb.remote_job_no_nzb (only accounts whose provider takes NZB files: list_remote_job_providers with container nzb), the indexer's refusal or a document that is no NZB (collector.nzb_*), and the provider's own refusal; the hit then stays waiting. `already_running` true means this account already had a job for this NZB and nothing was sent."
    )]
    pub async fn hand_over_subscription_item(
        &self,
        Parameters(params): Parameters<HandOverSubscriptionItemParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let account_id = parse_id(&params.account_id)?;
            Ok(
                crate::subscription_handlers::submit_subscription_item_remote_job(
                    State(self.state.clone()),
                    AxumPath(id),
                    Json(
                        crate::subscription_handlers::SubscriptionItemRemoteJobRequest {
                            account_id,
                        },
                    ),
                )
                .await?
                .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Answer the question a job in `awaiting_choice` asked, by naming the entry ids to fetch. Ids the job did not offer are dropped rather than forwarded."
    )]
    pub async fn choose_remote_job_entries(
        &self,
        Parameters(params): Parameters<RemoteJobChoiceParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            Ok(crate::remote_job_handlers::choose_remote_job_entries(
                State(self.state.clone()),
                AxumPath(id),
                Json(RemoteJobChoiceRequest {
                    entries: params.entries,
                }),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Remove one remote job from this installation's list. Nothing is sent to the provider and nothing is deleted there; the job keeps running on the provider's side. Deleting it at the provider is not available here and is done in the web UI, where it is confirmed."
    )]
    pub async fn forget_remote_job(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            Ok(crate::remote_job_handlers::forget_remote_job(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Remove remote jobs from this installation's list, all of them or the part one provider and a set of states select (`provider` a slug, `states` from awaiting_choice, ready, failed, discarded). Nothing is sent to any provider and nothing is deleted there; the jobs stay in the provider accounts. Deleting at the provider is not available here and is done in the web UI. Jobs still running at their provider (submitting, preparing, working) are always left out and listed under `skipped`. Irreversible for the list, so it asks first: a call without a `confirmation` code changes nothing and answers with a question for the person and a code; call again with confirmed=true and that code only after the person agreed. The answer counts `removed` and lists every job with its provider."
    )]
    pub async fn clear_remote_jobs(
        &self,
        Parameters(params): Parameters<ClearRemoteJobsParams>,
    ) -> McpToolResult {
        if let Some(question) =
            self.ask_first(
            "clear_remote_jobs",
            params.confirmation.as_deref(),
            "Remove the selected remote jobs from this installation's list (nothing is deleted at the provider).",
        )
        {
            return question;
        }
        let result = async {
            let states = params
                .states
                .iter()
                .map(|state| {
                    rd_core::RemoteJobState::from_str_value(state.trim()).ok_or_else(|| {
                        crate::ApiError::bad_request(
                            "remote_job.state_invalid",
                            "states are awaiting_choice, ready, failed or discarded",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(crate::remote_job_handlers::clear_remote_jobs(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(RemoteJobClearRequest {
                    confirmed: params.confirmed,
                    // Never at the provider over MCP (owner, 2026-09-23 and 2026-10-08).
                    at_provider: false,
                    provider: params.provider,
                    states,
                }),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }
}
