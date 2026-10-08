//! One rule for the tools that name a script (RD-1190-21).
//!
//! A script subscription is refused outright (`super::tools_intake::refuse_script`). The other
//! places a tool could name a script -- a package's or a LinkGrabber package's, a category's, an
//! automation's script action, the completion and the reconnect script of the settings -- each
//! choose among the files the administrator placed in the scripts folder, which REST allows on
//! purpose (`docs/security/scripts.md`). Through MCP an agent would choose what runs, so they
//! are refused unless the person switched `mcp_scripts_allowed` on, which no tool can do.
//!
//! What counts is a script the target does not already carry: clearing one starts nothing, and
//! a tool that passes back the name the person stored -- a replaced automation, a category's
//! post-processing sent whole -- changes nothing either.

use serde_json::{Map, Value};

use crate::{ApiError, AppState};

/// The stable code of a refused script.
pub(crate) const SCRIPT_NOT_ALLOWED: &str = "mcp.script_not_allowed";

/// Refuses `named` unless it is blank, what `stored` already is, or the person allowed scripts.
pub(crate) async fn check(
    state: &AppState,
    field: &'static str,
    named: Option<&str>,
    stored: Option<&str>,
) -> Result<(), ApiError> {
    let Some(name) = named.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(());
    };
    if stored.map(str::trim) == Some(name) {
        return Ok(());
    }
    if crate::settings_store::read_settings(state)
        .await?
        .mcp_scripts_allowed
    {
        return Ok(());
    }
    Err(refused(field))
}

/// The refusal, naming the field that carried the script.
pub(crate) fn refused(field: &'static str) -> ApiError {
    ApiError::forbidden(
        SCRIPT_NOT_ALLOWED,
        "Naming a script through a tool is switched off; the person allows it in the settings",
    )
    .with_param("field", field)
}

/// The top-level `script` of a passed-through REST body, if it is a string.
pub(crate) fn body_script(body: &Map<String, Value>) -> Option<&str> {
    body.get("script").and_then(Value::as_str)
}

/// The names of the script actions in an automation's `actions`, as the REST body spells them.
///
/// Read from JSON rather than from `rd_automation::Action`, which this crate does not depend
/// on; the request is deserialised from this very shape, so the two cannot disagree.
pub(crate) fn automation_scripts(actions: Option<&Value>) -> Vec<String> {
    actions
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|action| action.get("kind").and_then(Value::as_str) == Some("script"))
        .filter_map(|action| action.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Refuses an automation definition naming a script its stored version does not carry.
pub(crate) async fn check_automation(
    state: &AppState,
    definition: &Map<String, Value>,
    stored: &[String],
) -> Result<(), ApiError> {
    for name in automation_scripts(definition.get("actions")) {
        let known = stored.iter().any(|kept| kept.trim() == name.trim());
        check(
            state,
            "actions",
            Some(name.as_str()),
            known.then_some(name.as_str()),
        )
        .await?;
    }
    Ok(())
}

/// The script actions of an automation's newest stored version.
pub(crate) async fn stored_automation_scripts(
    state: &AppState,
    id: rd_core::AutomationId,
) -> Result<Vec<String>, ApiError> {
    let newest = state
        .database
        .automation_versions(id)
        .await?
        .into_iter()
        .max_by_key(|version| version.version);
    let actions = newest
        .map(|version| serde_json::to_value(&version.actions))
        .transpose()
        .map_err(anyhow::Error::new)?;
    Ok(automation_scripts(actions.as_ref()))
}

/// Refuses a settings change that names a completion or reconnect script not stored yet.
pub(crate) async fn check_settings(
    state: &AppState,
    current: &crate::dto::SettingsResponse,
    next: &crate::dto::SettingsResponse,
) -> Result<(), ApiError> {
    check(
        state,
        "completion_script",
        next.completion_script.as_deref(),
        current.completion_script.as_deref(),
    )
    .await?;
    check(
        state,
        "reconnect_script",
        next.reconnect_script.as_deref(),
        current.reconnect_script.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{automation_scripts, body_script, refused};

    #[test]
    fn the_script_actions_are_read_from_the_rest_body() {
        let actions = serde_json::json!([
            { "kind": "pause_queue" },
            { "kind": "script", "name": "notify.sh" },
            { "kind": "webhook", "name": "not-a-script" },
            { "kind": "script", "name": "tidy.py" },
        ]);
        assert_eq!(
            automation_scripts(Some(&actions)),
            vec!["notify.sh".to_owned(), "tidy.py".to_owned()]
        );
        assert!(automation_scripts(None).is_empty());
        assert!(automation_scripts(Some(&serde_json::json!({}))).is_empty());
    }

    #[test]
    fn a_body_s_script_is_its_top_level_string() {
        let body = serde_json::json!({ "script": "unpack.sh", "nested": { "script": "x" } });
        let serde_json::Value::Object(map) = body else {
            panic!("an object");
        };
        assert_eq!(body_script(&map), Some("unpack.sh"));
        assert_eq!(body_script(&serde_json::Map::new()), None);
    }

    #[test]
    fn the_refusal_names_the_field_with_a_stable_code() {
        let message = refused("script").into_message();
        assert_eq!(message.code, super::SCRIPT_NOT_ALLOWED);
        assert_eq!(
            message.params.get("field").map(String::as_str),
            Some("script")
        );
    }
}
