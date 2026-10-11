//! Web Push for the installed app (RD-1240-13): the key a browser subscribes with, and the
//! browsers that receive push messages.
//!
//! A browser turns push on under Settings > Interface: it asks for the public key, subscribes at
//! its own push service and hands the subscription over here. The first subscription also makes
//! the `web_push` notification target and a rule for every event (`rd-db`'s `web_push_store`);
//! each browser's own choice of events filters what reaches it.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
};
use rd_notify::{NotificationEvent, WebPushSubscription};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// The longest push address a subscription may name; the push services hand out a few hundred
/// characters.
const MAX_ENDPOINT_CHARS: usize = 2048;

/// The longest device name.
const MAX_DEVICE_CHARS: usize = 100;

/// What a device without a name is called.
const UNNAMED_DEVICE: &str = "Browser";

/// The key a browser subscribes with (`applicationServerKey`).
#[derive(Serialize, ToSchema)]
pub struct WebPushKeyResponse {
    /// URL-safe base64 of the uncompressed P-256 public key.
    pub public_key: String,
}

/// A browser's message keys, as `PushSubscription.toJSON()` names them.
#[derive(Deserialize, ToSchema)]
pub struct WebPushKeys {
    pub p256dh: String,
    pub auth: String,
}

/// A browser's push subscription. The same `endpoint` again updates the stored one.
#[derive(Deserialize, ToSchema)]
pub struct WebPushSubscriptionRequest {
    /// The push service's `https` address for this browser.
    pub endpoint: String,
    pub keys: WebPushKeys,
    /// What the device is called in the list; empty is "Browser".
    #[serde(default)]
    pub device_name: String,
    /// The events this browser wants; empty means every event.
    #[serde(default)]
    pub events: Vec<NotificationEvent>,
}

/// The public key browsers subscribe with, made the first time anybody asks.
#[utoipa::path(get, path = "/api/v1/notifications/web-push/key", tag = "notifications", responses((status = 200, body = WebPushKeyResponse)))]
pub async fn web_push_key(
    State(state): State<AppState>,
) -> Result<Json<WebPushKeyResponse>, ApiError> {
    let key = rd_api_core::web_push::vapid_key(&state.database, &state.secrets).await?;
    Ok(Json(WebPushKeyResponse {
        public_key: key.public_key(),
    }))
}

/// Every browser that receives push messages. Their message keys are never returned.
#[utoipa::path(get, path = "/api/v1/notifications/web-push/subscriptions", tag = "notifications", responses((status = 200, body = [WebPushSubscription])))]
pub async fn list_web_push_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<Vec<WebPushSubscription>>, ApiError> {
    Ok(Json(state.database.list_web_push_subscriptions().await?))
}

/// Stores a browser's subscription, or updates the one with the same push address.
#[utoipa::path(post, path = "/api/v1/notifications/web-push/subscriptions", tag = "notifications", request_body = WebPushSubscriptionRequest, responses((status = 201, body = WebPushSubscription), (status = 400)))]
pub async fn create_web_push_subscription(
    State(state): State<AppState>,
    Json(request): Json<WebPushSubscriptionRequest>,
) -> Result<(StatusCode, Json<WebPushSubscription>), ApiError> {
    let endpoint = request.endpoint.trim();
    // The address rule of the sending side: an address on the person's own network or this
    // machine is refused here, not at the first message (RD-1240-28).
    if endpoint.chars().count() > MAX_ENDPOINT_CHARS
        || !rd_notify::is_deliverable_push_address(endpoint, &rd_http::SystemLookup).await
    {
        return Err(ApiError::bad_request(
            "notification.push_endpoint_invalid",
            "A push subscription needs the push service's public https address",
        ));
    }
    let (p256dh, auth) = (request.keys.p256dh.trim(), request.keys.auth.trim());
    if !rd_notify::are_push_keys(p256dh, auth) {
        return Err(ApiError::bad_request(
            "notification.push_keys_invalid",
            "The subscription's keys are not a browser's push keys",
        ));
    }
    let device_name = match request.device_name.trim() {
        "" => UNNAMED_DEVICE.to_owned(),
        name if name.chars().count() > MAX_DEVICE_CHARS => {
            return Err(ApiError::bad_request(
                "notification.push_device_invalid",
                "A device name may be at most 100 characters",
            ));
        }
        name => name.to_owned(),
    };
    let subscription = state
        .database
        .upsert_web_push_subscription(rd_db::NewWebPushSubscription {
            endpoint: endpoint.to_owned(),
            p256dh: p256dh.to_owned(),
            auth: auth.to_owned(),
            device_name,
            events: request.events,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(subscription)))
}

/// Stops push messages to one browser. The browser itself unsubscribes at its push service.
#[utoipa::path(delete, path = "/api/v1/notifications/web-push/subscriptions/{id}", tag = "notifications", params(("id" = String, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_web_push_subscription(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .delete_web_push_subscription(&id)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "notification.push_subscription_not_found",
                "Push subscription not found",
            )
        })?;
    Ok(Json(crate::dto::MessageResponse::new(
        "notification.push_subscription_deleted",
        "Push subscription deleted",
    )))
}
