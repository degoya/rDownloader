//! Switching a bandwidth profile on by hand, in front of the schedule (RD-190-20).
//!
//! The qBittorrent "alternative speed" switch, with an end: until the schedule's next change,
//! until a chosen time, or until it is switched back. Editing profiles and the schedule stays
//! where it was; this only picks one of them for a while.

use axum::{Json, extract::State};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{
    AppState,
    bandwidth_handlers::{BandwidthStatusResponse, bandwidth_status},
    error::ApiError,
};

/// The furthest a chosen end may lie ahead; "until I switch back" is the open-ended choice.
const MAX_DAYS_AHEAD: i64 = 30;

#[derive(Deserialize, ToSchema)]
pub struct ManualProfileRequest {
    /// The profile to switch to; empty for no limits at all.
    #[serde(default)]
    pub profile_id: Option<rd_core::BandwidthProfileId>,
    /// `next_switch` (the schedule's next change), `at` (the time in `until`) or `never` (only
    /// switching back ends it).
    pub ends: rd_limits::ManualEnd,
    /// The end for `ends: at`; ignored otherwise.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
}

/// Switches to a profile by hand until the chosen end, then answers the status.
#[utoipa::path(put, path = "/api/v1/bandwidth/manual", tag = "bandwidth", request_body = ManualProfileRequest, responses((status = 200, body = BandwidthStatusResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn switch_bandwidth_profile(
    State(state): State<AppState>,
    Json(request): Json<ManualProfileRequest>,
) -> Result<Json<BandwidthStatusResponse>, ApiError> {
    if let Some(id) = request.profile_id {
        let profiles = state.database.list_bandwidth_profiles().await?;
        if !profiles.iter().any(|profile| profile.id == id) {
            return Err(ApiError::bad_request(
                "bandwidth.profile_not_found",
                "Bandwidth profile not found",
            ));
        }
    }
    let at = match request.ends {
        rd_limits::ManualEnd::At => Some(end_ahead(request.until)?),
        rd_limits::ManualEnd::NextSwitch | rd_limits::ManualEnd::Never => None,
    };
    state
        .scheduler
        .switch_bandwidth_profile(request.profile_id, request.ends, at)
        .await?;
    bandwidth_status(State(state)).await
}

/// Ends a switch made by hand, so the schedule decides again; answers the status.
#[utoipa::path(delete, path = "/api/v1/bandwidth/manual", tag = "bandwidth", responses((status = 200, body = BandwidthStatusResponse)))]
pub async fn return_to_bandwidth_schedule(
    State(state): State<AppState>,
) -> Result<Json<BandwidthStatusResponse>, ApiError> {
    state.scheduler.return_to_bandwidth_schedule().await?;
    bandwidth_status(State(state)).await
}

/// A chosen end that lies ahead, and not further than [`MAX_DAYS_AHEAD`].
fn end_ahead(until: Option<DateTime<Utc>>) -> Result<DateTime<Utc>, ApiError> {
    let now = Utc::now();
    until
        .filter(|until| *until > now && *until <= now + Duration::days(MAX_DAYS_AHEAD))
        .ok_or_else(|| {
            ApiError::bad_request(
                "bandwidth.manual_end_invalid",
                "The end must lie ahead, within 30 days",
            )
            .with_param("max_days", MAX_DAYS_AHEAD)
        })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::end_ahead;

    #[test]
    fn an_end_must_lie_ahead_and_within_thirty_days() {
        let now = Utc::now();
        assert!(end_ahead(Some(now + Duration::hours(1))).is_ok());
        assert!(end_ahead(None).is_err());
        assert!(end_ahead(Some(now - Duration::minutes(1))).is_err());
        assert!(end_ahead(Some(now + Duration::days(31))).is_err());
    }
}
