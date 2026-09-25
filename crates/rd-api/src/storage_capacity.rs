//! Free-space status and the manual release of a blocked storage root (RD-050-15).

use axum::{Json, extract::Path as AxumPath, extract::State};
use rd_files::StorageTarget;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// Path segment addressing the service download directory, which has no root id.
const FALLBACK_SEGMENT: &str = "fallback";

/// Why a target is holding back work; all four numbers are shown verbatim in the UI.
#[derive(Serialize, ToSchema)]
pub struct CapacityShortfallResponse {
    pub required_bytes: rd_core::ByteCount,
    pub free_bytes: rd_core::ByteCount,
    pub minimum_free_bytes: rd_core::ByteCount,
    /// `false` when no runner could state a size and the headroom policy was applied.
    pub size_known: bool,
}

#[derive(Serialize, ToSchema)]
pub struct StorageCapacityEntry {
    /// `None` for the service download directory, which is not a configured root.
    pub storage_root_id: Option<rd_core::StorageRootId>,
    /// Path segment for the resume action.
    pub target: String,
    pub name: String,
    pub path: String,
    pub free_bytes: Option<rd_core::ByteCount>,
    pub total_bytes: Option<rd_core::ByteCount>,
    /// Effective threshold: the root's own value, else the global default.
    pub minimum_free_bytes: rd_core::ByteCount,
    pub blocked: bool,
    pub shortfall: Option<CapacityShortfallResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct StorageCapacityResponse {
    pub auto_resume: bool,
    pub unknown_size_headroom: u32,
    pub roots: Vec<StorageCapacityEntry>,
}

#[utoipa::path(get, path = "/api/v1/storage/capacity", tag = "system", responses((status = 200, body = StorageCapacityResponse)))]
pub async fn storage_capacity(
    State(state): State<AppState>,
) -> Result<Json<StorageCapacityResponse>, ApiError> {
    let capacity = state.scheduler.capacity();
    let settings = capacity.settings().await;
    let names = state
        .database
        .list_storage_roots()
        .await?
        .into_iter()
        .map(|root| (root.id, root.name))
        .collect::<std::collections::HashMap<_, _>>();
    let mut roots = Vec::new();
    for (target, path) in capacity.targets().await {
        let minimum = capacity.minimum_free_bytes(&path).await;
        let probe = path.clone();
        let (free, total) = tokio::task::spawn_blocking(move || {
            (
                fs2::available_space(&probe).ok(),
                fs2::total_space(&probe).ok(),
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("storage probe task failed: {error}"))?;
        let shortfall = capacity.shortfall(target).await;
        roots.push(StorageCapacityEntry {
            storage_root_id: match target {
                StorageTarget::Root(id) => Some(id),
                StorageTarget::Fallback => None,
            },
            target: segment_of(target),
            name: match target {
                StorageTarget::Root(id) => names.get(&id).cloned().unwrap_or_default(),
                StorageTarget::Fallback => "Downloads".to_owned(),
            },
            path: path.to_string_lossy().into_owned(),
            free_bytes: free.and_then(|value| rd_core::ByteCount::new(value).ok()),
            total_bytes: total.and_then(|value| rd_core::ByteCount::new(value).ok()),
            minimum_free_bytes: rd_core::ByteCount::new(minimum).unwrap_or_default(),
            blocked: shortfall.is_some(),
            shortfall: shortfall.map(|shortfall| CapacityShortfallResponse {
                required_bytes: rd_core::ByteCount::new(shortfall.required_bytes)
                    .unwrap_or_default(),
                free_bytes: rd_core::ByteCount::new(shortfall.free_bytes).unwrap_or_default(),
                minimum_free_bytes: rd_core::ByteCount::new(shortfall.minimum_free_bytes)
                    .unwrap_or_default(),
                size_known: shortfall.size_known,
            }),
        });
    }
    Ok(Json(StorageCapacityResponse {
        auto_resume: settings.storage_auto_resume,
        unknown_size_headroom: settings.storage_unknown_size_headroom,
        roots,
    }))
}

/// Releases a blocked target and requeues exactly the downloads it held back.
#[utoipa::path(post, path = "/api/v1/storage/capacity/{target}/resume", tag = "system", params(("target" = String, Path, description = "Storage root id or `fallback`")), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn resume_storage(
    State(state): State<AppState>,
    AxumPath(target): AxumPath<String>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    let target = parse_target(&target)?;
    let released = state.scheduler.resume_storage(target).await?;
    Ok(Json(if released {
        crate::dto::MessageResponse::new("storage.resumed", "Storage root released")
    } else {
        crate::dto::MessageResponse::new("storage.not_blocked", "Storage root was not blocked")
    }))
}

/// The shortfall holding back new work for a destination, if its root is blocked.
///
/// Intake is stopped per root on purpose: a full media disk must not keep links for another
/// root out of the queue.
pub(crate) async fn intake_block(
    capacity: &rd_files::CapacityService,
    destination: &std::path::Path,
) -> Option<rd_files::CapacityShortfall> {
    let target = capacity.target_for(destination).await;
    capacity.shortfall(target).await
}

/// REST form of the intake stop.
pub(crate) async fn ensure_intake_allowed(
    capacity: &rd_files::CapacityService,
    destination: &std::path::Path,
) -> Result<(), ApiError> {
    let Some(shortfall) = intake_block(capacity, destination).await else {
        return Ok(());
    };
    Err(ApiError::conflict(
        "storage.capacity_blocked",
        "The storage root is below its free-space threshold and takes no new downloads",
    )
    .with_param("path", destination.to_string_lossy())
    .with_param("free_bytes", shortfall.free_bytes)
    .with_param("minimum_free_bytes", shortfall.minimum_free_bytes))
}

fn segment_of(target: StorageTarget) -> String {
    match target {
        StorageTarget::Root(id) => id.to_string(),
        StorageTarget::Fallback => FALLBACK_SEGMENT.to_owned(),
    }
}

fn parse_target(value: &str) -> Result<StorageTarget, ApiError> {
    if value == FALLBACK_SEGMENT {
        return Ok(StorageTarget::Fallback);
    }
    value.parse::<rd_core::StorageRootId>().map_or_else(
        |_| {
            Err(ApiError::bad_request(
                "storage.target_invalid",
                "Storage target must be a storage root id or `fallback`",
            ))
        },
        |id| Ok(StorageTarget::Root(id)),
    )
}

#[cfg(test)]
mod tests {
    use rd_files::StorageTarget;

    use super::{parse_target, segment_of};

    #[test]
    fn a_target_survives_the_round_trip_through_its_path_segment() {
        let id = rd_core::StorageRootId::new();
        assert_eq!(
            parse_target(&segment_of(StorageTarget::Root(id))).expect("root"),
            StorageTarget::Root(id)
        );
        assert_eq!(
            parse_target(&segment_of(StorageTarget::Fallback)).expect("fallback"),
            StorageTarget::Fallback
        );
        assert!(parse_target("not-a-target").is_err());
    }
}
