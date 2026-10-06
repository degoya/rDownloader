//! The request bodies: the multipart form a job is created with, and a control call's JSON.

use super::control_id_field;
use crate::source::Kind;

/// The multipart boundary, derived from bytes the host's random source produced.
///
/// Random rather than fixed because a container is somebody else's file: a fixed boundary that
/// happened to occur inside an NZB would split the part in the middle and submit half a
/// document. An empty answer from the host is a refusal, not an invitation to invent one, so
/// the caller checks the length before this is reached.
#[must_use]
pub fn boundary(entropy: &[u8]) -> String {
    let mut text = String::from("rdownloader");
    for byte in entropy {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// One `multipart/form-data` body: text fields first, then at most one file part.
#[must_use]
pub fn multipart(
    boundary: &str,
    fields: &[(&str, &str)],
    file: Option<(&str, &str, &[u8])>,
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    if let Some((name, file_name, bytes)) = file {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// The `Content-Type` a [`multipart`] body is sent under.
#[must_use]
pub fn multipart_content_type(boundary: &str) -> String {
    format!("multipart/form-data; boundary={boundary}")
}

/// The JSON body a control endpoint takes.
#[must_use]
pub fn control_body(kind: Kind, remote_id: &str, operation: &str) -> Vec<u8> {
    // `remote_id` has passed `is_safe_remote_id`, so it carries no quote and no backslash and
    // needs no escaping; it is written through `serde_json` anyway rather than formatted, so
    // that the guarantee lives in one place instead of in every caller's head.
    let value = serde_json::json!({
        control_id_field(kind): remote_id,
        "operation": operation,
    });
    serde_json::to_vec(&value).unwrap_or_default()
}
