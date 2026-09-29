//! One mask over every tool answer (RD-120-57).
//!
//! An indexer hit's download address carries the indexer's API key in its query, because that
//! is how the file is fetched later; queued into the LinkGrabber, it becomes a candidate's
//! address and a download's source, in clear. The REST routes hand those rows to the owner's own
//! browser. A tool hands them to a model, which is the first of the owner's four marks.
//!
//! Masking each tool that happens to carry an address today would miss the next one, so every
//! answer passes through here on its way out of `RdMcpServer::call_tool`: every string, at any
//! depth -- text content, structured content, and a JSON-RPC error's message and data. A string
//! that is an address with a host loses its credentials through `rd_core::redact_url`; any other
//! string goes through `rd_core::redact_text`, which finds addresses, `Authorization` lines and
//! `Bearer` values inside prose. Both keep the parameter's name and replace only its value, and
//! both leave a string with nothing to hide byte-for-byte, so ids and every other value a later
//! tool takes as its input come out exactly as they went in.

use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};
use serde_json::Value;

/// The answer of one tool call, with every credential in an address masked.
pub(crate) fn mask_response(
    answer: Result<CallToolResponse, rmcp::ErrorData>,
) -> Result<CallToolResponse, rmcp::ErrorData> {
    match answer {
        Ok(CallToolResponse::Complete(mut result)) => {
            mask_result(&mut result);
            Ok(CallToolResponse::Complete(result))
        }
        // No tool produces the other two shapes; a future one would carry no tool output yet.
        Ok(other) => Ok(other),
        Err(mut error) => {
            if let Some(masked) = mask_string(&error.message) {
                error.message = masked.into();
            }
            if let Some(data) = error.data.as_mut() {
                mask_value(data);
            }
            Err(error)
        }
    }
}

fn mask_result(result: &mut CallToolResult) {
    for block in &mut result.content {
        if let ContentBlock::Text(content) = block {
            if let Some(masked) = mask_text(&content.text) {
                content.text = masked;
            }
        } else if let Ok(mut value) = serde_json::to_value(&*block)
            && mask_value(&mut value)
            && let Ok(masked) = serde_json::from_value(value)
        {
            *block = masked;
        }
    }
    if let Some(structured) = result.structured_content.as_mut() {
        mask_value(structured);
    }
}

/// A text block: nearly always a JSON document (`error::json_result`), walked value by value
/// so an address is recognised whole; anything else as prose.
fn mask_text(text: &str) -> Option<String> {
    match serde_json::from_str::<Value>(text) {
        Ok(mut value) => {
            if !mask_value(&mut value) {
                return None;
            }
            // `json_result` writes pretty JSON; an unchanged answer is never rewritten at all.
            serde_json::to_string_pretty(&value).ok()
        }
        Err(_) => mask_string(text),
    }
}

/// Masks every string in `value`, keys included; whether anything changed.
pub(crate) fn mask_value(value: &mut Value) -> bool {
    match value {
        Value::String(text) => match mask_string(text) {
            Some(masked) => {
                *text = masked;
                true
            }
            None => false,
        },
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| mask_value(item) | changed),
        Value::Object(map) => {
            let mut changed = false;
            if map.keys().any(|key| mask_string(key).is_some()) {
                let entries = std::mem::take(map);
                for (key, nested) in entries {
                    map.insert(mask_string(&key).unwrap_or(key), nested);
                }
                changed = true;
            }
            map.values_mut()
                .fold(changed, |changed, nested| mask_value(nested) | changed)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The masked string, or `None` when it has nothing to hide.
fn mask_string(text: &str) -> Option<String> {
    // Any scheme with a host, so `nntp://user:pass@host` and `davs://…?token=` are covered too,
    // which the prose scan's short list of schemes would not find.
    let masked = match url::Url::parse(text) {
        Ok(address) if address.has_host() => {
            let masked = rd_core::redact_url(&address);
            if masked == address.as_str() {
                return None;
            }
            masked
        }
        _ => rd_core::redact_text(text),
    };
    (masked != text).then_some(masked)
}

#[cfg(test)]
mod tests {
    use super::{mask_response, mask_value};
    use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};

    const KEY: &str = "0123456789abcdef";

    fn text_of(answer: Result<CallToolResponse, rmcp::ErrorData>) -> String {
        match answer {
            Ok(CallToolResponse::Complete(result)) => match &result.content[0] {
                ContentBlock::Text(content) => content.text.clone(),
                _ => panic!("a text block"),
            },
            _ => panic!("a complete answer"),
        }
    }

    #[test]
    fn an_address_loses_its_key_at_any_depth_and_an_id_stays() {
        let mut answer = serde_json::json!({
            "items": [{
                "id": "0192f0c4-0000-7000-8000-00000000abcd",
                "url": format!("https://indexer.example/api?t=get&id=7&apikey={KEY}"),
                "note": format!("fetched https://indexer.example/getnzb?r=1&token={KEY} ok"),
                "server": format!("nntp://user:{KEY}@news.example:563"),
            }],
            "plain": "https://example.com/a?page=2",
        });
        let before = answer.clone();
        assert!(mask_value(&mut answer));
        let rendered = answer.to_string();
        assert!(!rendered.contains(KEY), "{rendered}");
        assert_eq!(answer["items"][0]["id"], before["items"][0]["id"]);
        assert_eq!(answer["plain"], before["plain"]);
        assert!(rendered.contains("apikey="), "the name stays: {rendered}");
    }

    #[test]
    fn an_answer_with_nothing_to_hide_is_not_rewritten() {
        // Compact on purpose: re-serialising would make it pretty, so equality proves no rewrite.
        let text = r#"{"id":"abc","url":"https://Example.COM"}"#;
        let answer = Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into());
        assert_eq!(text_of(mask_response(answer)), text);
    }

    #[test]
    fn text_structured_content_and_errors_are_all_masked() {
        let address = format!("https://indexer.example/api?apikey={KEY}");
        let mut result = CallToolResult::success(vec![ContentBlock::text(format!(
            "{{\"url\":\"{address}\"}}"
        ))]);
        result.structured_content = Some(serde_json::json!({ "url": address }));
        let Ok(CallToolResponse::Complete(masked)) = mask_response(Ok(result.into())) else {
            panic!("a complete answer");
        };
        let rendered = serde_json::to_string(&masked).expect("serialises");
        assert!(!rendered.contains(KEY), "{rendered}");

        let prose = Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "could not fetch {address}"
        ))])
        .into());
        assert!(!text_of(mask_response(prose)).contains(KEY));

        let error = rmcp::ErrorData::internal_error(
            format!("failed on {address}"),
            Some(serde_json::json!({ "url": address })),
        );
        let Err(error) = mask_response(Err(error)) else {
            panic!("still an error");
        };
        let rendered = serde_json::to_string(&error).expect("serialises");
        assert!(!rendered.contains(KEY), "{rendered}");
    }
}
