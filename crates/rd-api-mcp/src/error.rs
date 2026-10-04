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

/// Converts an [`ApiError`] into an `is_error` tool result, preserving the stable code and the
/// parameters the code's text names (a limit, a version), exactly as the REST body carries them.
pub(crate) fn api_error(error: ApiError) -> CallToolResult {
    let message = error.into_message();
    let mut body = serde_json::json!({ "error": message.message, "code": message.code });
    if !message.params.is_empty() {
        body["params"] = serde_json::json!(message.params);
    }
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

#[cfg(test)]
mod tests {
    use rmcp::model::ContentBlock;

    use super::api_error;
    use crate::ApiError;

    fn body_of(error: ApiError) -> serde_json::Value {
        let result = api_error(error);
        assert_eq!(result.is_error, Some(true));
        match &result.content[0] {
            ContentBlock::Text(content) => {
                serde_json::from_str(&content.text).expect("a JSON error body")
            }
            _ => panic!("a text block"),
        }
    }

    #[test]
    fn an_error_keeps_its_code_and_params() {
        let body = body_of(
            ApiError::bad_request("request.bulk_range", "Between 1 and 500 ids")
                .with_param("max", 500),
        );
        assert_eq!(body["code"], "request.bulk_range");
        assert_eq!(body["error"], "Between 1 and 500 ids");
        assert_eq!(body["params"]["max"], "500");
    }

    #[test]
    fn an_error_without_params_has_no_params_field() {
        let body = body_of(ApiError::not_found(
            "package.not_found",
            "Package not found",
        ));
        assert_eq!(body["code"], "package.not_found");
        assert!(body.get("params").is_none(), "{body}");
    }
}
