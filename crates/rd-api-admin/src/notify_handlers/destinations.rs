//! The destination catalogue: the installed notification-destination plugins and the settings
//! their targets may hold (RD-170-09, free text since RD-1250-01). Split out of
//! `notify_handlers.rs` to keep it under the line limit.

use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;

/// One installed notification-destination plugin, for the target editor.
#[derive(Serialize, ToSchema)]
pub struct NotificationDestination {
    /// The value a target stores in `config.plugin_id`.
    pub plugin_id: String,
    /// Default display name. The interface prefers the plugin's own localised name.
    pub name: String,
    pub version: String,
    /// The plugin's message namespace: a setting's label is its code
    /// `<slug>.setting.<name>`, a choice's `<slug>.choice.<value>`.
    pub slug: String,
    /// What a target of this destination may be set to, stored in `config.settings`
    /// (RD-170-09).
    pub settings: Vec<NotificationDestinationSetting>,
}

/// One setting a notification destination offers.
#[derive(Serialize, ToSchema)]
pub struct NotificationDestinationSetting {
    /// The key in `config.settings`.
    pub name: String,
    /// Every value the setting accepts, in the order to offer them. Empty: the setting is free
    /// text (RD-1250-01), at most 256 characters on one line, or 4096 when `multiline`, without
    /// control characters but line breaks.
    pub choices: Vec<String>,
    /// Free text only: whether it holds several lines, one entry each.
    pub multiline: bool,
    /// What applies while the target leaves the setting alone; absent means it may stay unset.
    pub default: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/notifications/destinations", tag = "notifications", responses((status = 200, body = [NotificationDestination])))]
pub async fn list_destinations(
    State(state): State<AppState>,
) -> Json<Vec<NotificationDestination>> {
    Json(
        state
            .notifications
            .notifiers()
            .await
            .list()
            .into_iter()
            .map(|destination| NotificationDestination {
                plugin_id: destination.plugin_id,
                name: destination.name,
                version: destination.version,
                slug: destination.slug,
                settings: destination
                    .settings
                    .into_iter()
                    .map(|setting| NotificationDestinationSetting {
                        name: setting.name,
                        choices: setting.choices,
                        multiline: setting.multiline,
                        default: setting.default,
                    })
                    .collect(),
            })
            .collect(),
    )
}
