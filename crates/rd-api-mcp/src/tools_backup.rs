//! MCP tools for the scheduled full backup (RD-160-01) and its destinations, retention and
//! verification (RD-160-02).
//!
//! Reading the schedule and the history, changing the schedule, managing the destinations,
//! previewing retention, starting a run and verifying an archive are here. Setting the
//! passphrase is not: it takes a secret in, which the owner's line keeps out of the toolbox
//! (`mcp_coverage`), and a backup without one cannot be written at all. No destination tool
//! takes a credential either: a profile is named by its id, an rclone remote by its name.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_storage::{
        BackupArchivesParams, BackupDestinationParams, BackupDestinationUpdateParams,
        BackupIdParams, BackupRetentionParams, BackupScheduleParams,
    },
};
use crate::backup_destination_handlers::{
    self, ArchiveQuery, BackupDestinationRequest, RetentionPreviewQuery,
};
use crate::backup_handlers::{self, UpdateBackupConfigRequest};

fn destination_request(params: BackupDestinationParams) -> BackupDestinationRequest {
    BackupDestinationRequest {
        kind: params.kind,
        name: params.name,
        enabled: params.enabled,
        path: params.path,
        profile_id: params.profile_id,
        prefix: params.prefix,
        remote: params.remote,
        keep_last: params.keep_last,
        keep_days: params.keep_days,
    }
}

#[tool_router(router = backup_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the full backup's setup: whether the schedule is on, its cron expression and time zone, every destination with its retention and archive count, the verification schedule, whether a passphrase is set up (never the passphrase or the key), when it next runs, and whether a run is going on now."
    )]
    pub async fn get_backup_status(&self) -> McpToolResult {
        respond(
            backup_handlers::get_backup_config(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the last full backup runs, newest first: scheduled or manual, running/succeeded/failed/interrupted, the archive's name, size and SHA-256 and its parts, how each destination fared (attempts, location, archives retention removed), or the stable error code of a failed run."
    )]
    pub async fn list_backup_runs(&self) -> McpToolResult {
        respond(
            backup_handlers::list_backup_runs(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Change the full backup's schedule and its verification schedule. Switching it on needs a passphrase the person set up in the interface (backup.key_missing without one) and a destination (backup.destination_missing). A changed schedule is timed again from now."
    )]
    pub async fn update_backup_schedule(
        &self,
        Parameters(params): Parameters<BackupScheduleParams>,
    ) -> McpToolResult {
        respond(
            backup_handlers::update_backup_config(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(UpdateBackupConfigRequest {
                    enabled: params.enabled,
                    schedule: params.schedule,
                    timezone: params.timezone,
                    verify_schedule: params.verify_schedule,
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Start a full backup now. It runs in the background and answers at once with the run's row; follow it with list_backup_runs. Downloads keep running while it is written. backup.already_running when one is still going."
    )]
    pub async fn run_backup(&self) -> McpToolResult {
        respond(
            backup_handlers::run_backup(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
            )
            .await
            .map(|(_, Json(answer))| answer),
        )
    }

    #[tool(
        description = "Add a full backup destination: kind local (path: an absolute folder or mounted NAS share), object_storage (profile_id and prefix <bucket>/<folder>) or rclone (remote name:path, WebDAV included). Each destination gets its own copy of every archive and keeps keep_last archives and/or those younger than keep_days; the newest always stays."
    )]
    pub async fn create_backup_destination(
        &self,
        Parameters(params): Parameters<BackupDestinationParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::create_backup_destination(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(destination_request(params)),
            )
            .await
            .map(|(_, Json(answer))| answer),
        )
    }

    #[tool(
        description = "Replace a full backup destination by id with all its fields, the way create_backup_destination takes them. Switching off the last destination of a switched-on schedule is refused with backup.destination_last."
    )]
    pub async fn update_backup_destination(
        &self,
        Parameters(params): Parameters<BackupDestinationUpdateParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::update_backup_destination(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Path(params.id),
                Json(destination_request(params.destination)),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Remove a full backup destination by id. Its archives stay where they are; only this installation's record of them goes, so retention never touches them again."
    )]
    pub async fn delete_backup_destination(
        &self,
        Parameters(params): Parameters<BackupIdParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::delete_backup_destination(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Path(params.id),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Preview which archives a destination's retention keeps and which it would remove, with the stored policy or keep_last/keep_days given here. Nothing is deleted. Only archives this installation recorded there are ever candidates."
    )]
    pub async fn preview_backup_retention(
        &self,
        Parameters(params): Parameters<BackupRetentionParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::preview_backup_retention(
                State(self.state.clone()),
                Path(params.id),
                Query(RetentionPreviewQuery {
                    keep_last: params.keep_last,
                    keep_days: params.keep_days,
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the archives this installation wrote to its backup destinations, newest first: name, location, size, SHA-256, and the last verification's state and code."
    )]
    pub async fn list_backup_archives(
        &self,
        Parameters(params): Parameters<BackupArchivesParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::list_backup_archives(
                State(self.state.clone()),
                Query(ArchiveQuery {
                    destination_id: params.destination_id,
                }),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Verify one archive (id from list_backup_archives) where it lies: fetched back, compared with the size and SHA-256 it was written with, and opened with the backup key to check every part. Runs in the background; follow it with list_backup_verifications."
    )]
    pub async fn verify_backup_archive(
        &self,
        Parameters(params): Parameters<BackupIdParams>,
    ) -> McpToolResult {
        respond(
            backup_destination_handlers::verify_backup_archive(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Path(params.id),
            )
            .await
            .map(|(_, Json(answer))| answer),
        )
    }

    #[tool(
        description = "List the last archive verifications, newest first: running/passed/failed/interrupted, whether the content was checked or only the digest (an archive under an earlier passphrase), and the stable code of a failure (backup.verify_missing, backup.verify_digest_mismatch, backup.verify_damaged or the destination's own)."
    )]
    pub async fn list_backup_verifications(&self) -> McpToolResult {
        respond(
            backup_destination_handlers::list_backup_verifications(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }
}
