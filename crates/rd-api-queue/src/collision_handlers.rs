//! Collision policies per category and package, and the answers to `ask` prompts (RD-150-01).
//!
//! The global policy is `storage_collision_policy` in the settings document and is written
//! with the rest of it; these routes carry the two levels below it and the one question the
//! queue can ask about a file.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use chrono::{DateTime, Utc};
use rd_core::{
    CollisionDecision, CollisionPhase, CollisionPolicy, DownloadId, DownloadState,
    EffectiveCollisionPolicy, PackageId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, dto::MessageResponse, error::ApiError, error_codes::parse_id};

/// One category's or package's own policy.
#[derive(Serialize, ToSchema)]
pub struct ScopedCollisionPolicy {
    pub id: String,
    pub policy: CollisionPolicy,
}

#[derive(Serialize, ToSchema)]
pub struct CollisionPoliciesResponse {
    /// The settings document's `storage_collision_policy`.
    pub global: CollisionPolicy,
    pub categories: Vec<ScopedCollisionPolicy>,
    pub packages: Vec<ScopedCollisionPolicy>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetCollisionPolicyRequest {
    /// `null` removes the level's own policy, so it inherits again.
    #[serde(default)]
    pub policy: Option<CollisionPolicy>,
}

/// Every level of one package, and the one that decides.
#[derive(Serialize, ToSchema)]
pub struct PackageCollisionPolicyResponse {
    pub package_id: PackageId,
    pub own: Option<CollisionPolicy>,
    pub category: Option<CollisionPolicy>,
    pub global: CollisionPolicy,
    pub effective: EffectiveCollisionPolicy,
}

/// A download waiting for an answer, or holding one until its next attempt.
#[derive(Serialize, ToSchema)]
pub struct CollisionPromptResponse {
    pub download_id: DownloadId,
    pub package_id: Option<PackageId>,
    pub package_name: Option<String>,
    /// The name that was taken, inside the package folder.
    pub target_name: String,
    pub phase: CollisionPhase,
    pub existing_bytes: Option<u64>,
    pub decision: Option<CollisionDecision>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, ToSchema)]
pub struct CollisionDecisionRequest {
    pub decision: CollisionDecision,
}

async fn global_policy(state: &AppState) -> Result<CollisionPolicy, ApiError> {
    Ok(state
        .database
        .service_settings_or_default::<rd_core::StorageSettings>()
        .await?
        .storage_collision_policy)
}

#[utoipa::path(get, path = "/api/v1/collision-policies", tag = "downloads", responses((status = 200, body = CollisionPoliciesResponse)))]
pub async fn list_collision_policies(
    State(state): State<AppState>,
) -> Result<Json<CollisionPoliciesResponse>, ApiError> {
    let mut categories = Vec::new();
    let mut packages = Vec::new();
    for row in state.database.list_collision_policies().await? {
        let entry = ScopedCollisionPolicy {
            id: row.scope_id,
            policy: row.policy,
        };
        if row.scope_kind == rd_db::COLLISION_SCOPE_CATEGORY {
            categories.push(entry);
        } else {
            packages.push(entry);
        }
    }
    Ok(Json(CollisionPoliciesResponse {
        global: global_policy(&state).await?,
        categories,
        packages,
    }))
}

#[utoipa::path(put, path = "/api/v1/categories/{id}/collision-policy", tag = "configuration", params(("id" = String, Path)), request_body = SetCollisionPolicyRequest, responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn set_category_collision_policy(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<SetCollisionPolicyRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<rd_core::CategoryId>(&id)?;
    if !state
        .database
        .list_categories()
        .await?
        .iter()
        .any(|category| category.id == id)
    {
        return Err(ApiError::not_found(
            "category.not_found",
            "Category not found",
        ));
    }
    state
        .database
        .set_category_collision_policy(id, request.policy)
        .await?;
    Ok(Json(MessageResponse::new(
        "collision.policy_saved",
        "Collision policy saved",
    )))
}

async fn package_view(
    state: &AppState,
    package_id: PackageId,
) -> Result<PackageCollisionPolicyResponse, ApiError> {
    let levels = state.database.collision_policy_levels(package_id).await?;
    let global = global_policy(state).await?;
    Ok(PackageCollisionPolicyResponse {
        package_id,
        own: levels.package,
        category: levels.category,
        global,
        effective: rd_core::effective_collision_policy(levels.package, levels.category, global),
    })
}

async fn ensure_package(state: &AppState, id: PackageId) -> Result<(), ApiError> {
    if state
        .database
        .list_packages()
        .await?
        .iter()
        .any(|package| package.id == id)
    {
        Ok(())
    } else {
        Err(crate::error_codes::package_not_found())
    }
}

#[utoipa::path(get, path = "/api/v1/packages/{id}/collision-policy", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = PackageCollisionPolicyResponse), (status = 404)))]
pub async fn get_package_collision_policy(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<PackageCollisionPolicyResponse>, ApiError> {
    let id = parse_id::<PackageId>(&id)?;
    ensure_package(&state, id).await?;
    Ok(Json(package_view(&state, id).await?))
}

#[utoipa::path(put, path = "/api/v1/packages/{id}/collision-policy", tag = "downloads", params(("id" = String, Path)), request_body = SetCollisionPolicyRequest, responses((status = 200, body = PackageCollisionPolicyResponse), (status = 404)))]
pub async fn set_package_collision_policy(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<SetCollisionPolicyRequest>,
) -> Result<Json<PackageCollisionPolicyResponse>, ApiError> {
    let id = parse_id::<PackageId>(&id)?;
    ensure_package(&state, id).await?;
    state
        .database
        .set_package_collision_policy(id, request.policy)
        .await?;
    Ok(Json(package_view(&state, id).await?))
}

#[utoipa::path(get, path = "/api/v1/collision-prompts", tag = "downloads", responses((status = 200, body = [CollisionPromptResponse])))]
pub async fn list_collision_prompts(
    State(state): State<AppState>,
) -> Result<Json<Vec<CollisionPromptResponse>>, ApiError> {
    let prompts = state.database.list_collision_prompts().await?;
    if prompts.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let downloads = state.database.list_downloads().await?;
    let packages = state.database.list_packages().await?;
    Ok(Json(
        prompts
            .into_iter()
            .map(|prompt| {
                let package_id = downloads
                    .iter()
                    .find(|file| file.id == prompt.download_id)
                    .map(|file| file.package_id);
                let package_name = package_id.and_then(|id| {
                    packages
                        .iter()
                        .find(|package| package.id == id)
                        .map(|package| package.name.clone())
                });
                CollisionPromptResponse {
                    download_id: prompt.download_id,
                    package_id,
                    package_name,
                    target_name: prompt.target_name,
                    phase: prompt.phase,
                    existing_bytes: prompt.existing_bytes,
                    decision: prompt.decision,
                    created_at: prompt.created_at,
                    decided_at: prompt.decided_at,
                }
            })
            .collect(),
    ))
}

/// Answers a prompt and lets the download go on. The answer is recorded before anything is
/// carried out, and carried out by the download's next attempt, which re-checks it: an
/// `overwrite` of a file that is in use by then is asked again rather than performed.
#[utoipa::path(post, path = "/api/v1/downloads/{id}/collision-decision", tag = "downloads", params(("id" = String, Path)), request_body = CollisionDecisionRequest, responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn decide_collision(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<CollisionDecisionRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    let download = state
        .database
        .get_download(id)
        .await?
        .ok_or_else(crate::error_codes::download_not_found)?;
    if !state
        .database
        .decide_collision_prompt(id, request.decision)
        .await?
    {
        return Err(ApiError::conflict(
            "collision.no_prompt",
            "This download is not waiting for a collision decision",
        ));
    }
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::CollisionDecided)
            .by(&audit)
            .target("download", id)
            .named(download.file_name.clone())
            .detail("decision", request.decision.as_str()),
    )
    .await;
    if download.state == DownloadState::Blocked {
        state.scheduler.resume(id).await?;
    }
    Ok(Json(MessageResponse::new(
        "collision.decided",
        "Decision recorded; the download continues",
    )))
}
