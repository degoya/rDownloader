//! REST surface of the automation engine (RD-090-04, RD-090-05).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_automation::{Automation, AutomationVersion, Run, Trigger};
use rd_core::{AutomationId, PackageId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    automation_input::{AutomationRequest, validated},
    automation_service::{DryRunDraft, DryRunMatch},
};

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
    /// The automation as it stands in the editor (RD-1120-17). When present only it is judged,
    /// whether or not it is saved or switched on, and the answer holds its one entry.
    #[serde(default)]
    pub draft: Option<DryRunDraft>,
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
    let mut definitions =
        crate::automation_service::current_definitions(&state.database, &automations).await?;
    let response: Vec<AutomationResponse> = automations
        .into_iter()
        .map(|automation| AutomationResponse {
            definition: definitions.remove(&automation.id),
            automation,
        })
        .collect();
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

/// Judges a trigger and a sample package against the enabled automations, or against the
/// editor's draft alone when the request carries one (RD-1120-17). Never has an effect.
#[utoipa::path(post, path = "/api/v1/automations/dry-run", tag = "automations", request_body = DryRunRequest, responses((status = 200, body = [DryRunMatch]), (status = 400), (status = 404)))]
pub async fn dry_run_automations(
    State(state): State<AppState>,
    Json(request): Json<DryRunRequest>,
) -> Result<Json<Vec<DryRunMatch>>, ApiError> {
    let missing = |error: anyhow::Error| {
        ApiError::not_found("automation.dry_run_target_missing", error.to_string())
            .with_param("reason", error)
    };
    let Some(draft) = request.draft else {
        return state
            .automations
            .dry_run(request.trigger, request.package_id, None)
            .await
            .map(Json)
            .map_err(missing);
    };
    // Refused with the code a save would answer: a draft whose condition cannot be evaluated
    // would otherwise read as a condition that does not hold.
    rd_automation::validate_condition(&draft.condition)
        .map_err(|error| ApiError::bad_request(error.code(), error.to_string()))?;
    let matched = state
        .automations
        .dry_run_draft(request.trigger, request.package_id, draft)
        .await
        .map_err(missing)?;
    Ok(Json(vec![matched]))
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

fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(&error, "automation.not_found", "Automation not found")
}
