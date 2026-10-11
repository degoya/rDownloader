//! An automation definition as a person submits it, and the one validation it passes through —
//! shared by the REST surface and the area bundle import, so a bundle cannot carry a definition
//! the editor would refuse.

use rd_automation::{Action, ConditionNode, Schedule, Trigger};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::ApiError;

/// A definition being created or replaced.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AutomationRequest {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub trigger: Trigger,
    /// When a `schedule` trigger runs (RD-1240-10): `{"kind":"cron","expression":"0 6 * * *"}`
    /// or `{"kind":"interval","minutes":60}`, read in the service's time zone. Ignored for
    /// every other trigger.
    #[serde(default)]
    pub schedule: Option<Schedule>,
    #[serde(default)]
    pub condition: ConditionNode,
    pub actions: Vec<Action>,
}

/// Validates a request and turns it into the store's input.
///
/// Validation happens here rather than in the store so an invalid definition is refused with
/// the stable code the web client translates, and never reaches a state where the engine has
/// to decide at trigger time what an unevaluatable rule means.
pub fn validated(request: AutomationRequest) -> Result<rd_db::NewAutomation, ApiError> {
    let refused = |error: rd_automation::DefinitionError| {
        ApiError::bad_request(error.code(), error.to_string())
    };
    rd_automation::validate(&request.name, &request.condition, &request.actions)
        .map_err(refused)?;
    rd_automation::validate_trigger(
        request.trigger,
        request.schedule.as_ref(),
        &request.actions,
        chrono::Utc::now(),
        &chrono::Local,
    )
    .map_err(refused)?;
    // A schedule left over from a trigger the editor switched away from means nothing; the
    // stored version carries one only where it is read.
    let schedule = request
        .schedule
        .filter(|_| request.trigger == Trigger::Schedule);
    Ok(rd_db::NewAutomation {
        name: request.name.trim().to_owned(),
        enabled: request.enabled,
        trigger: request.trigger,
        schedule,
        condition: request.condition,
        actions: request.actions,
    })
}
