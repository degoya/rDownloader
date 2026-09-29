//! An automation definition as a person submits it, and the one validation it passes through —
//! shared by the REST surface and the area bundle import, so a bundle cannot carry a definition
//! the editor would refuse.

use rd_automation::{Action, ConditionNode, Trigger};
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
