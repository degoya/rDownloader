//! Livestream endpoints: monitored channels (CRUD) and immediate recordings.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::{StreamChannel, StreamChannelId};
use rd_db::NewStreamChannel;
use url::Url;

use crate::{
    ApiError, AppState,
    dto::{RecordNowRequest, StreamChannelRequest},
};

/// Normalises and validates the editable channel fields.
pub(crate) fn channel_input(request: StreamChannelRequest) -> Result<NewStreamChannel, ApiError> {
    let url = Url::parse(request.url.trim())
        .map_err(|error| ApiError::bad_request("stream.url_invalid", error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "stream.url_invalid",
            "Channel URL must use http or https",
        ));
    }
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "stream".to_owned());
    if name.len() > 120 {
        return Err(ApiError::bad_request(
            "stream.name_too_long",
            "Channel name must be at most 120 characters",
        )
        .with_param("max", 120));
    }
    let quality = request
        .quality
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if quality.as_ref().is_some_and(|value| value.len() > 50) {
        return Err(ApiError::bad_request(
            "stream.quality_invalid",
            "Stream quality must be at most 50 characters",
        ));
    }
    // Refused rather than clamped: a split bound of zero would cut on every tick, and a
    // person who typed it should be told, not silently given something else.
    if !request.recording.is_valid() {
        return Err(ApiError::unprocessable(
            "stream.recording_policy_invalid",
            "That recording policy cannot be applied",
        ));
    }
    Ok(NewStreamChannel {
        url: url.to_string(),
        name,
        quality,
        category_id: request.category_id,
        enabled: request.enabled,
        recording: request.recording,
    })
}

#[utoipa::path(get, path = "/api/v1/streams/channels", tag = "streams", responses((status = 200, body = [StreamChannel])))]
pub async fn list_stream_channels(
    State(state): State<AppState>,
) -> Result<Json<Vec<StreamChannel>>, ApiError> {
    Ok(Json(state.database.list_stream_channels().await?))
}

#[utoipa::path(post, path = "/api/v1/streams/channels", tag = "streams", request_body = StreamChannelRequest, responses((status = 201, body = StreamChannel)))]
pub async fn create_stream_channel(
    State(state): State<AppState>,
    Json(request): Json<StreamChannelRequest>,
) -> Result<(StatusCode, Json<StreamChannel>), ApiError> {
    let channel = state
        .database
        .create_stream_channel(channel_input(request)?)
        .await?;
    Ok((StatusCode::CREATED, Json(channel)))
}

#[utoipa::path(put, path = "/api/v1/streams/channels/{id}", tag = "streams", params(("id" = StreamChannelId, Path)), request_body = StreamChannelRequest, responses((status = 200, body = StreamChannel), (status = 404)))]
pub async fn update_stream_channel(
    State(state): State<AppState>,
    Path(id): Path<StreamChannelId>,
    Json(request): Json<StreamChannelRequest>,
) -> Result<Json<StreamChannel>, ApiError> {
    let channel = state
        .database
        .update_stream_channel(id, channel_input(request)?)
        .await
        .map_err(not_found)?;
    Ok(Json(channel))
}

#[utoipa::path(delete, path = "/api/v1/streams/channels/{id}", tag = "streams", params(("id" = StreamChannelId, Path)), responses((status = 204), (status = 404)))]
pub async fn delete_stream_channel(
    State(state): State<AppState>,
    Path(id): Path<StreamChannelId>,
) -> Result<StatusCode, ApiError> {
    state
        .database
        .delete_stream_channel(id)
        .await
        .map_err(not_found)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/streams/record", tag = "streams", request_body = RecordNowRequest, responses((status = 201, body = rd_core::DownloadPackage)))]
pub async fn record_now(
    State(state): State<AppState>,
    Json(request): Json<RecordNowRequest>,
) -> Result<(StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let input = channel_input(StreamChannelRequest {
        url: request.url,
        name: request.name,
        quality: request.quality,
        category_id: request.category_id,
        enabled: true,
        // "Record now" carries no policy of its own; the runner picks up the channel's by
        // address, so a saved channel's splitting and sidecars still apply.
        recording: rd_core::RecordingPolicy::default(),
    })?;
    let settings = state.stream_settings.read().await.clone();
    let package = crate::stream_monitor::start_recording(
        &state.database,
        &state.scheduler,
        &input.url,
        &input.name,
        input.quality.as_deref(),
        input.category_id,
        &settings,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(package)))
}

fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(
        &error,
        "stream.channel_not_found",
        "Stream channel not found",
    )
}
