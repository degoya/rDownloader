//! A rule's own wording (RD-1280-02): the placeholder table the rule editor lists and previews
//! with, and the check a template passes when the rule is saved. Split out of
//! `notify_handlers.rs` to keep it under the line limit.

use axum::Json;
use rd_notify::{NotificationEvent, Placeholder, TemplateError, TemplatePart};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;

/// The placeholders a rule's title and text may name, and which event fills which.
#[derive(Serialize, ToSchema)]
pub struct NotificationPlaceholders {
    /// Every placeholder, with a sample value for the editor's preview; `time` and `date` are
    /// the service's clock, in its time zone and format.
    pub placeholders: Vec<NotificationPlaceholder>,
    /// Every event a rule can name, with the placeholders it fills.
    pub events: Vec<NotificationEventPlaceholders>,
}

/// One placeholder, written `{name}` in a template.
#[derive(Serialize, ToSchema)]
pub struct NotificationPlaceholder {
    pub name: Placeholder,
    pub example: String,
}

/// What one event fills.
#[derive(Serialize, ToSchema)]
pub struct NotificationEventPlaceholders {
    pub event: NotificationEvent,
    /// False for the events whose bursts arrive as one summary: they keep their own text.
    pub takes_template: bool,
    /// Empty when `takes_template` is false.
    pub placeholders: Vec<Placeholder>,
}

#[utoipa::path(get, path = "/api/v1/notifications/placeholders", tag = "notifications", responses((status = 200, body = NotificationPlaceholders)))]
pub async fn list_notification_placeholders() -> Json<NotificationPlaceholders> {
    let now = chrono::Local::now();
    let example = |placeholder: Placeholder| match placeholder {
        Placeholder::Time => now.format("%H:%M").to_string(),
        Placeholder::Date => now.format("%Y-%m-%d").to_string(),
        other => other.example().to_owned(),
    };
    Json(NotificationPlaceholders {
        placeholders: Placeholder::ALL
            .into_iter()
            .map(|name| NotificationPlaceholder {
                name,
                example: example(name),
            })
            .collect(),
        events: NotificationEvent::all()
            .into_iter()
            .map(|event| NotificationEventPlaceholders {
                event,
                takes_template: event.takes_template(),
                placeholders: event.placeholders(),
            })
            .collect(),
    })
}

/// A template as the rule stores it: `None` for one left out or blank, which keeps the
/// built-in wording.
///
/// # Errors
///
/// `notification.template_syntax`, `notification.template_unknown_placeholder` (with the
/// `placeholder`) or `notification.template_too_long` (with the `max`), each naming the request
/// `field`.
pub fn checked_template(
    part: TemplatePart,
    template: Option<String>,
) -> Result<Option<String>, ApiError> {
    let Some(template) = template.filter(|text| !text.trim().is_empty()) else {
        return Ok(None);
    };
    rd_notify::validate_template(part, &template).map_err(|error| {
        let refused = ApiError::bad_request(error.code(), error.to_string())
            .with_param("field", part.field());
        match error {
            TemplateError::UnknownPlaceholder { name } => refused.with_param("placeholder", name),
            TemplateError::TooLong { max } => refused.with_param("max", max),
            TemplateError::Syntax => refused,
        }
    })?;
    Ok(Some(template))
}
