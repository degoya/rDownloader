//! One change of the settings document, from reading the stored one to saving its successor
//! (RD-1480-07): what `PUT`, `PATCH`, the log level and the MCP tool `update_settings` share.

use axum::http::HeaderMap;
use rd_api_core::settings_change;
use serde_json::{Map, Value};
use tokio::sync::OwnedMutexGuard;

use super::{apply_holding_turn, privileged_change};
use crate::{ApiError, AppState, dto::SettingsResponse, settings_store::read_settings};

/// The stored document, held against every other writer until it is saved or dropped.
pub struct SettingsEdit<'a> {
    state: &'a AppState,
    current: SettingsResponse,
    _turn: OwnedMutexGuard<()>,
}

impl<'a> SettingsEdit<'a> {
    /// Waits for the writers before this one and reads what they stored.
    pub async fn begin(state: &'a AppState) -> Result<Self, ApiError> {
        let turn = state.admin.settings_turn.take().await;
        Ok(Self {
            state,
            current: read_settings(state).await?,
            _turn: turn,
        })
    }

    /// The stored document this change starts from.
    #[must_use]
    pub fn current(&self) -> &SettingsResponse {
        &self.current
    }

    /// Refuses the change when the request's `If-Match` names another document.
    pub fn require_match(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        settings_change::require_match(headers, &self.current)
    }

    /// The stored document with `patch` laid over it.
    ///
    /// A merge patch of the document's top level: a field left out keeps its value, a field named
    /// is replaced whole, an object or a list in it included. `null` is the empty value of a field
    /// that can be empty; any other field refuses it, and never falls back to its default.
    pub fn patched(&self, patch: Map<String, Value>) -> Result<SettingsResponse, ApiError> {
        merge(&self.current, patch)
    }

    /// Saves `settings` on a caller's behalf: the privileged-field gate, the apply, the event and
    /// the audit record, in the one function both the REST routes and the MCP tool call.
    ///
    /// The gate used to live in the REST handler alone, and the MCP tool `update_settings` went
    /// straight to the apply. Both cost `api:config`, so a configuration token could switch the
    /// administrator login off -- and with it gain every scope -- over MCP while the same change
    /// over REST was refused (RD-130-09). One function is what keeps the two from drifting apart
    /// again. `holds_admin` is the caller's own grant, never the service's.
    pub async fn save_settings(
        self,
        audit: &crate::audit::AuditContext,
        holds_admin: bool,
        settings: SettingsResponse,
    ) -> Result<SettingsResponse, ApiError> {
        let Self { state, current, .. } = &self;
        if !holds_admin && let Some(field) = privileged_change(current, &settings) {
            // Audited as a failure: a refused attempt to widen what a credential may reach is
            // exactly the thing somebody reads an audit log to find.
            crate::audit::record(
                state,
                crate::audit::AuditEvent::failure(rd_core::AuditAction::SettingsChanged)
                    .by(audit)
                    .target("settings", "service.settings")
                    .detail("refused_field", field)
                    .detail("reason", "scope_insufficient"),
            )
            .await;
            return Err(ApiError::forbidden(
                "auth.scope_insufficient",
                "This setting requires the administration scope",
            )
            .with_param("scope", rd_core::Scope::Admin.as_str())
            .with_param("setting", field));
        }
        // Only a changed target: a stored one whose profile went since must not hold up every
        // other setting on the page (RD-1190-20).
        if settings.upload_remote != current.upload_remote {
            rd_api_core::object_upload_target::check_object_upload_target(
                &state.database,
                settings.upload_remote.as_deref(),
            )
            .await?;
        }
        let (applied, torrent_refused) = apply_holding_turn(state, Some(current), settings).await?;
        // The *names* of the fields that changed, never their values: the settings document holds
        // secret references, executable paths and proxy addresses, and an audit log that quoted
        // them would be a copy of the configuration with a timestamp on it.
        crate::audit::record(
            state,
            crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsChanged)
                .by(audit)
                .target("settings", "service.settings")
                .detail(
                    "fields",
                    settings_change::changed_fields(current, &applied).join(" "),
                ),
        )
        .await;
        // Saved and audited, but the torrent engine kept its previous settings: said, not a
        // silent `200` beside a status that shows the new values (RD-1240-28).
        if let Some(refused) = torrent_refused {
            return Err(refused);
        }
        Ok(applied)
    }
}

/// [`SettingsEdit::patched`] over a document in hand.
fn merge(
    current: &SettingsResponse,
    patch: Map<String, Value>,
) -> Result<SettingsResponse, ApiError> {
    if patch.is_empty() {
        return Err(ApiError::bad_request(
            "settings.patch_empty",
            "The patch must contain at least one settings key",
        ));
    }
    let Value::Object(mut merged) = serde_json::to_value(current).map_err(anyhow::Error::new)?
    else {
        return Err(anyhow::anyhow!("settings did not serialize to an object").into());
    };
    let known = known_fields()?;
    let named: Vec<String> = patch.keys().cloned().collect();
    for (key, value) in patch {
        if !known.contains_key(&key) {
            return Err(ApiError::bad_request(
                "settings.unknown_key",
                format!("Unknown settings key: {key}"),
            )
            .with_param("key", key));
        }
        merged.insert(key, value);
    }
    serde_json::from_value(Value::Object(merged.clone())).map_err(|error| {
        // serde names the type it wanted and not the field it wanted it for.
        let refused = named.into_iter().find(|key| {
            let alone = merged.get_key_value(key);
            let alone = alone.map(|(key, value)| (key.clone(), value.clone()));
            let alone = Value::Object(alone.into_iter().collect());
            serde_json::from_value::<SettingsResponse>(alone).is_err()
        });
        match refused {
            Some(key) => ApiError::bad_request("settings.invalid", format!("{key}: {error}"))
                .with_param("key", key),
            None => ApiError::bad_request("settings.invalid", error.to_string()),
        }
    })
}

/// Every field the document has. Read from its schema rather than from a serialised document: a
/// field that is left out while it is empty (`media_output_template`) is a field all the same.
fn known_fields() -> Result<Map<String, Value>, ApiError> {
    let schema = serde_json::to_value(<SettingsResponse as utoipa::PartialSchema>::schema())
        .map_err(anyhow::Error::new)?;
    match schema.get("properties") {
        Some(Value::Object(properties)) => Ok(properties.clone()),
        _ => Err(anyhow::anyhow!("the settings schema lists no properties").into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{SettingsResponse, known_fields, merge};

    fn patch(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(patch) => patch,
            other => panic!("not an object: {other}"),
        }
    }

    fn stored() -> SettingsResponse {
        SettingsResponse {
            max_retries: 7,
            // Neither is the default, so a patch that reset them would show.
            external_url: Some("https://downloads.example".to_owned()),
            media_output_template: Some("%(title)s".to_owned()),
            ..SettingsResponse::default()
        }
    }

    #[test]
    fn a_field_left_out_keeps_its_value() {
        let merged = merge(&stored(), patch(json!({ "max_active_files": 9 }))).expect("merged");
        assert_eq!(merged.max_active_files, 9);
        assert_eq!(merged.max_retries, 7);
        assert_eq!(
            merged.external_url.as_deref(),
            Some("https://downloads.example")
        );
    }

    /// `null` empties a field that can be empty, whether or not the document writes the empty
    /// value down.
    #[test]
    fn null_empties_a_field_that_can_be_empty() {
        let merged = merge(
            &stored(),
            patch(json!({ "external_url": null, "media_output_template": null })),
        )
        .expect("merged");
        assert_eq!(merged.external_url, None);
        assert_eq!(merged.media_output_template, None);
        assert_eq!(merged.max_retries, 7);
    }

    /// Anywhere else it is refused by name, and never read as "back to the default".
    #[test]
    fn null_is_refused_for_a_field_that_cannot_be_empty() {
        for field in ["max_retries", "admin_login_disabled", "trusted_proxies"] {
            let Err(refused) = merge(&stored(), patch(json!({ field: null }))) else {
                panic!("{field} took null");
            };
            assert_eq!(refused.code(), "settings.invalid", "{field}");
            assert!(refused.message().contains(field), "{}", refused.message());
        }
    }

    /// An object is one value: the two levels are not merged, so a client can drop an entry.
    #[test]
    fn a_nested_object_is_replaced_whole() {
        let mut current = stored();
        current.package_name_rules = serde_json::from_value(json!({
            "spaces_to_dots": true,
            "collapse_separators": true,
        }))
        .expect("rules");
        let merged = merge(
            &current,
            patch(json!({ "package_name_rules": { "spaces_to_dots": true } })),
        )
        .expect("merged");
        let rules = serde_json::to_value(merged.package_name_rules).expect("rules");
        assert_eq!(rules["spaces_to_dots"], json!(true));
        assert_eq!(rules["collapse_separators"], json!(false));
    }

    #[test]
    fn an_empty_patch_and_an_unknown_field_are_refused() {
        let refused = merge(&stored(), Map::new()).err().expect("empty");
        assert_eq!(refused.code(), "settings.patch_empty");
        let refused = merge(&stored(), patch(json!({ "max_retires": 1 })))
            .err()
            .expect("unknown");
        assert_eq!(refused.code(), "settings.unknown_key");
    }

    /// Every field a stored document carries is known, the ones it leaves out while empty too.
    #[test]
    fn the_known_fields_are_the_documents() {
        let known = known_fields().expect("fields");
        let Value::Object(document) = serde_json::to_value(stored()).expect("serialize") else {
            panic!("not an object");
        };
        for field in document.keys() {
            assert!(known.contains_key(field), "{field}");
        }
        assert!(known.contains_key("media_default_criteria"));
    }
}
