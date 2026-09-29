//! Mapping between REST-layer errors and MCP tool results.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

use crate::ApiError;

pub(crate) type McpToolResult = Result<CallToolResult, rmcp::ErrorData>;

/// Serializes a success payload as pretty JSON text content.
pub(crate) fn json_result(value: &impl Serialize) -> McpToolResult {
    let text = serde_json::to_string_pretty(value).map_err(|error| {
        rmcp::ErrorData::internal_error(format!("failed to serialize tool result: {error}"), None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Converts an [`ApiError`] into an `is_error` tool result, preserving the stable code.
pub(crate) fn api_error(error: ApiError) -> CallToolResult {
    let body = serde_json::json!({ "error": error.message(), "code": error.code() });
    CallToolResult::error(vec![ContentBlock::text(body.to_string())])
}

/// Bridges `Result<T, ApiError>` into a tool result without losing error codes.
pub(crate) fn respond<T: Serialize>(result: Result<T, ApiError>) -> McpToolResult {
    match result {
        Ok(value) => json_result(&value),
        Err(error) => Ok(api_error(error)),
    }
}

/// The REST layer's id parser, under the name the tool modules already use.
pub(crate) use crate::error_codes::parse_id;

/// Parses a whole list of typed ids.
pub(crate) fn parse_ids<T>(values: &[String]) -> Result<Vec<T>, ApiError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    values.iter().map(|value| parse_id(value)).collect()
}

/// Property names a tool must never accept, because they carry a credential.
///
/// The vault is filled in the web UI. A tool that took a password would put it in a model's
/// context, in a transcript and in whatever the client logs — three places it was never meant
/// to be — and the `definition` passthroughs would be exactly the hole through which one could
/// arrive, since their body is only checked by the REST handler afterwards.
pub(crate) const CREDENTIAL_FIELDS: &[&str] = &[
    "api_key",
    "cookies",
    "credential",
    "passphrase",
    "password",
    "secret",
    "token",
];

/// Refuses a passed-through request body that names a credential field, at any depth.
pub(crate) fn without_credentials(
    body: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    fn scan(value: &serde_json::Value) -> Result<(), ApiError> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, nested) in map {
                    if CREDENTIAL_FIELDS.contains(&key.as_str()) {
                        return Err(ApiError::bad_request(
                            "request.credential_rejected",
                            format!("{key} is a credential and cannot be set through a tool call"),
                        )
                        .with_param("field", key.clone()));
                    }
                    scan(nested)?;
                }
                Ok(())
            }
            serde_json::Value::Array(items) => items.iter().try_for_each(scan),
            _ => Ok(()),
        }
    }
    scan(&serde_json::Value::Object(body.clone()))?;
    Ok(body)
}

/// Deserializes a passed-through REST body, mapping a shape mistake onto a stable code.
pub(crate) fn from_definition<T: serde::de::DeserializeOwned>(
    body: serde_json::Map<String, serde_json::Value>,
) -> Result<T, ApiError> {
    let body = without_credentials(body)?;
    serde_json::from_value(serde_json::Value::Object(body))
        .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))
}
