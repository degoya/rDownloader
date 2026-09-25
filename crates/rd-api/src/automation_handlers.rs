//! REST surface of the automation engine (RD-090-04, RD-090-05).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_automation::{Action, Automation, AutomationVersion, ConditionNode, Run, Trigger};
use rd_core::{AutomationId, PackageId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, automation_service::DryRunMatch};

/// An automation together with the definition currently in force.
///
/// Returned as one object because an editor needs both and a second round trip to fetch the
/// definition of every listed automation is the kind of thing that makes a list page slow.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationResponse {
    #[serde(flatten)]
    pub automation: Automation,
    pub definition: Option<AutomationVersion>,
}

/// A definition being created or replaced.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AutomationRequest {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default)]
    pub condition: ConditionNode,
    pub actions: Vec<Action>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnableRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DryRunRequest {
    pub trigger: Trigger,
    /// A real package to judge the condition against; `None` evaluates against nothing,
    /// which is what a condition-free automation is.
    #[serde(default)]
    pub package_id: Option<PackageId>,
}

#[derive(Debug, Deserialize)]
pub struct RunQuery {
    #[serde(default)]
    automation_id: Option<AutomationId>,
    #[serde(default)]
    limit: Option<u32>,
}

/// The vocabulary the editor builds its forms from.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationVocabulary {
    pub triggers: Vec<Trigger>,
    pub fields: Vec<&'static str>,
    pub operators: Vec<&'static str>,
    pub action_kinds: Vec<&'static str>,
    pub max_actions: usize,
    pub max_condition_depth: usize,
}

#[utoipa::path(get, path = "/api/v1/automations", tag = "automations", responses((status = 200, body = [AutomationResponse])))]
pub async fn list_automations(
    State(state): State<AppState>,
) -> Result<Json<Vec<AutomationResponse>>, ApiError> {
    let automations = state.database.list_automations().await?;
    let mut response = Vec::with_capacity(automations.len());
    for automation in automations {
        let definition = state
            .database
            .automation_versions(automation.id)
            .await?
            .into_iter()
            .find(|version| version.version == automation.version);
        response.push(AutomationResponse {
            automation,
            definition,
        });
    }
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/v1/automations", tag = "automations", request_body = AutomationRequest, responses((status = 201, body = Automation), (status = 400)))]
pub async fn create_automation(
    State(state): State<AppState>,
    Json(request): Json<AutomationRequest>,
) -> Result<(StatusCode, Json<Automation>), ApiError> {
    let input = validated(request)?;
    Ok((
        StatusCode::CREATED,
        Json(state.database.upsert_automation(None, input).await?),
    ))
}

#[utoipa::path(put, path = "/api/v1/automations/{id}", tag = "automations", params(("id" = AutomationId, Path)), request_body = AutomationRequest, responses((status = 200, body = Automation), (status = 400), (status = 404)))]
pub async fn update_automation(
    State(state): State<AppState>,
    Path(id): Path<AutomationId>,
    Json(request): Json<AutomationRequest>,
) -> Result<Json<Automation>, ApiError> {
    let input = validated(request)?;
    Ok(Json(
        state
            .database
            .upsert_automation(Some(id), input)
            .await
            .map_err(not_found)?,
    ))
}

#[utoipa::path(post, path = "/api/v1/automations/{id}/enable", tag = "automations", params(("id" = AutomationId, Path)), request_body = EnableRequest, responses((status = 200, body = Automation), (status = 404)))]
pub async fn enable_automation(
    State(state): State<AppState>,
    Path(id): Path<AutomationId>,
    Json(request): Json<EnableRequest>,
) -> Result<Json<Automation>, ApiError> {
    Ok(Json(
        state
            .database
            .set_automation_enabled(id, request.enabled)
            .await
            .map_err(not_found)?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/automations/{id}", tag = "automations", params(("id" = AutomationId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_automation(
    State(state): State<AppState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .delete_automation(id)
        .await
        .map_err(not_found)?;
    Ok(Json(crate::dto::MessageResponse::new(
        "automation.deleted",
        "Automation deleted",
    )))
}

#[utoipa::path(get, path = "/api/v1/automations/{id}/versions", tag = "automations", params(("id" = AutomationId, Path)), responses((status = 200, body = [AutomationVersion])))]
pub async fn list_automation_versions(
    State(state): State<AppState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Vec<AutomationVersion>>, ApiError> {
    Ok(Json(state.database.automation_versions(id).await?))
}

#[utoipa::path(get, path = "/api/v1/automations/runs", tag = "automations", responses((status = 200, body = [Run])))]
pub async fn list_automation_runs(
    State(state): State<AppState>,
    Query(query): Query<RunQuery>,
) -> Result<Json<Vec<Run>>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(crate::automation_service::DEFAULT_HISTORY)
        .clamp(1, 500);
    Ok(Json(
        state
            .database
            .automation_runs(query.automation_id, limit)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/automations/dry-run", tag = "automations", request_body = DryRunRequest, responses((status = 200, body = [DryRunMatch]), (status = 404)))]
pub async fn dry_run_automations(
    State(state): State<AppState>,
    Json(request): Json<DryRunRequest>,
) -> Result<Json<Vec<DryRunMatch>>, ApiError> {
    state
        .automations
        .dry_run(request.trigger, request.package_id, None)
        .await
        .map(Json)
        .map_err(|error| {
            ApiError::not_found("automation.dry_run_target_missing", error.to_string())
        })
}

#[utoipa::path(get, path = "/api/v1/automations/vocabulary", tag = "automations", responses((status = 200, body = AutomationVocabulary)))]
pub async fn automation_vocabulary() -> Json<AutomationVocabulary> {
    Json(AutomationVocabulary {
        triggers: Trigger::all().to_vec(),
        fields: vec![
            "source",
            "domain",
            "extension",
            "name",
            "category",
            "state",
            "failure_code",
            "size_bytes",
            "kind",
        ],
        operators: vec![
            "equals",
            "contains",
            "starts_with",
            "ends_with",
            "matches",
            "greater_than",
            "less_than",
        ],
        action_kinds: vec![
            "webhook",
            "script",
            "set_category",
            "pause_package",
            "resume_package",
        ],
        max_actions: rd_automation::MAX_ACTIONS,
        max_condition_depth: rd_automation::MAX_CONDITION_DEPTH,
    })
}

/// Validates a request and turns it into the store's input.
///
/// Validation happens here rather than in the store so an invalid definition is refused with
/// the stable code the web client translates, and never reaches a state where the engine has
/// to decide at trigger time what an unevaluatable rule means.
pub(crate) fn validated(request: AutomationRequest) -> Result<rd_db::NewAutomation, ApiError> {
    rd_automation::validate(&request.name, &request.condition, &request.actions)
        .map_err(|error| ApiError::bad_request(error.code(), error.to_string()))?;
    Ok(rd_db::NewAutomation {
        name: request.name.trim().to_owned(),
        enabled: request.enabled,
        trigger: request.trigger,
        condition: request.condition,
        actions: request.actions,
    })
}

fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(&error, "automation.not_found", "Automation not found")
}
