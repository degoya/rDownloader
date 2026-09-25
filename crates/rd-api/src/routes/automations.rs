//! Automation definitions, versions, run history and the editor's dry run.

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, automation_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/automations",
            get(automation_handlers::list_automations).post(automation_handlers::create_automation),
        )
        .route(
            "/api/v1/automations/runs",
            get(automation_handlers::list_automation_runs),
        )
        .route(
            "/api/v1/automations/vocabulary",
            get(automation_handlers::automation_vocabulary),
        )
        .route(
            "/api/v1/automations/export",
            get(crate::area_backup::export_automations),
        )
        .route(
            "/api/v1/automations/import",
            post(crate::area_backup::import_automations),
        )
        .route(
            "/api/v1/automations/dry-run",
            post(automation_handlers::dry_run_automations),
        )
        .route(
            "/api/v1/automations/{id}",
            axum::routing::put(automation_handlers::update_automation)
                .delete(automation_handlers::delete_automation),
        )
        .route(
            "/api/v1/automations/{id}/enable",
            post(automation_handlers::enable_automation),
        )
        .route(
            "/api/v1/automations/{id}/versions",
            get(automation_handlers::list_automation_versions),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(
    paths(
        automation_handlers::list_automations,
        automation_handlers::create_automation,
        automation_handlers::update_automation,
        automation_handlers::delete_automation,
        automation_handlers::enable_automation,
        automation_handlers::list_automation_versions,
        automation_handlers::list_automation_runs,
        automation_handlers::dry_run_automations,
        crate::area_backup::export_automations,
        crate::area_backup::import_automations,
        automation_handlers::automation_vocabulary,
    ),
    components(schemas(
        automation_handlers::AutomationResponse,
        automation_handlers::AutomationRequest,
        automation_handlers::AutomationVocabulary,
        automation_handlers::DryRunRequest,
        automation_handlers::EnableRequest,
        crate::automation_service::DryRunMatch,
        rd_automation::Action,
        rd_automation::Automation,
        rd_automation::AutomationVersion,
        rd_automation::ConditionNode,
        rd_automation::Field,
        rd_automation::Operator,
        rd_automation::Predicate,
        rd_automation::Run,
        rd_automation::RunState,
        rd_automation::Trigger,
    ))
)]
pub(crate) struct Doc;
