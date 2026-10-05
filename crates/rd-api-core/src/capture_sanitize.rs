//! Boundary validation for the request metadata of intercepted browser downloads.
//!
//! Capture clients are expected to filter headers themselves; this module is the
//! second line of defence. Policy-disallowed content (unknown or credential-bearing
//! headers, non-web effective URLs) is dropped, structurally broken input is rejected.

use base64::{Engine, engine::general_purpose::STANDARD};
use rd_core::{
    CapturedBody, CapturedHeader, CapturedRequest, MAX_BODY_FIELD_NAMES, MAX_CAPTURED_HEADER_NAME,
    MAX_CAPTURED_HEADERS, MAX_CAPTURED_VALUE, MAX_REPLAY_BODY_B64, MAX_REPLAY_BODY_BYTES,
    ReplayBlockReason, ReplayBodyKind, ReplayMethod, derive_approved_origins,
    is_allowed_captured_header, is_credential_header, signed_url_expiry,
};
use url::Url;

use crate::ApiError;

/// `400` when a client sends more links than one batch may carry.
#[must_use]
pub fn links_limit(max: usize) -> ApiError {
    ApiError::bad_request(
        "capture.links_limit",
        format!("Provide at most {max} links per capture batch"),
    )
    .with_param("max", max)
}

/// `400` for a request method other than `GET` or `POST`.
#[must_use]
pub fn method_unsupported() -> ApiError {
    ApiError::bad_request(
        "capture.method_unsupported",
        "Only GET and POST requests can be captured",
    )
}

/// `400` when a `GET` carries a request body, which it never legitimately does.
#[must_use]
pub fn body_not_allowed() -> ApiError {
    ApiError::bad_request(
        "capture.body_not_allowed",
        "A GET request cannot carry a body",
    )
}

/// `400` when the encoded body exceeds what the contract accepts.
#[must_use]
pub fn body_length(max: usize) -> ApiError {
    ApiError::bad_request(
        "capture.body_length",
        format!("Provide at most {max} bytes of encoded request body"),
    )
    .with_param("max", max)
}

/// `400` when the body is not valid base64.
#[must_use]
pub fn body_invalid() -> ApiError {
    ApiError::bad_request("capture.body_invalid", "Request body is not valid base64")
}

/// `400` when a captured request carries more headers than allowed.
#[must_use]
pub fn headers_limit(max: usize) -> ApiError {
    ApiError::bad_request(
        "capture.headers_limit",
        format!("Provide at most {max} request headers"),
    )
    .with_param("max", max)
}

/// `400` for an oversized header name or value.
#[must_use]
pub fn header_length() -> ApiError {
    ApiError::bad_request("capture.header_length", "Request header is too long")
}

/// `400` for an oversized referrer, user agent or content disposition.
#[must_use]
pub fn field_length(field: &str) -> ApiError {
    ApiError::bad_request(
        "capture.field_length",
        format!("Captured `{field}` is too long"),
    )
    .with_param("field", field)
}

/// `400` for a structured link whose URL cannot be parsed, named by its place in the batch.
///
/// **It takes the position and not the address, on purpose** (RD-109-39). A mistyped address of
/// a protected share is exactly an address that fails to parse, and a share password rides in
/// the fragment -- so the string handed to this function is the likely carrier of a secret.
/// Returning it redacted is not an option either: redaction works on a parsed `Url`, and this is
/// precisely the string no parser would take, so nothing is known about where its fragment
/// begins or whether its `@` separates a user name from a host. A guess is the wrong ground for
/// a secret, so the function never sees the string at all. `position` is 1-based: the caller
/// still holds the text it just sent, and what it cannot know is which of its links was refused.
#[must_use]
pub fn link_url_invalid(position: usize) -> ApiError {
    ApiError::bad_request(
        "collector.link_url_invalid",
        format!("Link {position} in the batch is not a valid URL"),
    )
    .with_param("position", position)
}

/// Body bytes a caller must move into the secret store, together with the sanitized request.
///
/// Returned rather than stored here so this function stays synchronous and unit-testable;
/// the caller owns the vault reference and its cleanup.
pub type SanitizedRequest = (CapturedRequest, Option<Vec<u8>>);

/// Body metadata kept with the capture, the decoded bytes to vault, and the reason replay is
/// impossible when it is.
type SanitizedBody = (
    Option<CapturedBody>,
    Option<Vec<u8>>,
    Option<ReplayBlockReason>,
);

/// Validates and strips a captured request down to the allowlisted contract.
///
/// Two classes of problem, deliberately handled differently:
///
/// * **Structurally broken** input (an unsupported method, a body that is not base64, a
///   header over the limit) is rejected with a stable code. The client sent something the
///   contract does not describe.
/// * **Policy-unreproducible** input (a file upload, an oversize body, a `multipart` type)
///   is *accepted* with `replayable = false` and a `blocked_reason`. The capture is valid;
///   it just cannot be replayed. Rejecting it instead would make the link vanish, because
///   the extension has already cancelled the browser's own download by this point — the
///   user would be left with nothing and no explanation.
pub fn sanitize(url: &Url, request: CapturedRequest) -> Result<SanitizedRequest, ApiError> {
    if request.headers.len() > MAX_CAPTURED_HEADERS {
        return Err(headers_limit(MAX_CAPTURED_HEADERS));
    }
    let Some(method) = ReplayMethod::parse(&request.method) else {
        return Err(method_unsupported());
    };
    let method = method.as_str().to_owned();
    let mut headers = Vec::new();
    for header in request.headers {
        let name = header.name.trim().to_ascii_lowercase();
        if is_credential_header(&name) {
            // A client that filters correctly never sends these; log the contract breach.
            tracing::warn!(header = %name, "dropped credential header from capture payload");
            continue;
        }
        if !is_allowed_captured_header(&name) {
            continue;
        }
        if name.len() > MAX_CAPTURED_HEADER_NAME || header.value.len() > MAX_CAPTURED_VALUE {
            return Err(header_length());
        }
        headers.push(CapturedHeader {
            name,
            value: header.value.trim().to_owned(),
        });
    }
    let effective_url = request.effective_url.filter(|url| {
        let web = matches!(url.scheme(), "http" | "https");
        if !web {
            tracing::warn!(scheme = url.scheme(), "dropped non-web effective URL");
        }
        web
    });
    let (body, body_bytes, mut blocked_reason) =
        sanitize_body(&method, &headers, request.body_b64, request.has_file_upload)?;

    // Server-derived from here on: a client value for any of these is ignored, so a
    // compromised or buggy capture client cannot widen its own replay permissions.
    let approved_origins = derive_approved_origins(url, effective_url.as_ref());
    let expires_at = signed_url_expiry(effective_url.as_ref().unwrap_or(url));
    if blocked_reason.is_none() && expires_at.is_some_and(|deadline| deadline <= chrono::Utc::now())
    {
        blocked_reason = Some(ReplayBlockReason::Expired);
    }

    Ok((
        CapturedRequest {
            effective_url,
            method,
            referrer: text_field(request.referrer, "referrer")?,
            user_agent: text_field(request.user_agent, "user_agent")?,
            content_disposition: text_field(request.content_disposition, "content_disposition")?,
            headers,
            body,
            // Cleared unconditionally: the plaintext body never reaches persistence.
            body_b64: None,
            has_file_upload: request.has_file_upload,
            expires_at,
            approved_origins,
            replayable: blocked_reason.is_none(),
            blocked_reason,
        },
        body_bytes,
    ))
}

/// Validates the request body and decides whether it can be stored for replay.
///
/// Returns the metadata kept with the capture, the decoded bytes the caller must vault, and
/// the reason replay is impossible when it is.
fn sanitize_body(
    method: &str,
    headers: &[CapturedHeader],
    body_b64: Option<String>,
    has_file_upload: bool,
) -> Result<SanitizedBody, ApiError> {
    let Some(encoded) = body_b64.filter(|value| !value.is_empty()) else {
        // An empty-body POST is legitimate and reproducible; only a POST whose body was
        // lost on the way here is not, and the client signals that by sending none.
        return Ok((None, None, None));
    };
    if method != "POST" {
        return Err(body_not_allowed());
    }
    if encoded.len() > MAX_REPLAY_BODY_B64 {
        return Err(body_length(MAX_REPLAY_BODY_B64));
    }
    let decoded = STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| body_invalid())?;

    let content_type = headers
        .iter()
        .find(|header| header.name == "content-type")
        .map(|header| header.value.clone())
        .unwrap_or_default();
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    // Refuse first, describe second: every refusal still produces metadata so the UI can
    // explain the block instead of showing an inert link.
    let refusal = if has_file_upload {
        Some(ReplayBlockReason::FileUpload)
    } else if essence.starts_with("multipart/") {
        Some(ReplayBlockReason::MultipartUnsupported)
    } else if ReplayBodyKind::from_content_type(&content_type).is_none() {
        Some(ReplayBlockReason::ContentTypeUnsupported)
    } else if decoded.len() > MAX_REPLAY_BODY_BYTES {
        Some(ReplayBlockReason::BodyTooLarge)
    } else {
        None
    };

    let kind = ReplayBodyKind::from_content_type(&content_type).unwrap_or(ReplayBodyKind::Text);
    let field_names = if kind == ReplayBodyKind::FormUrlencoded && refusal.is_none() {
        form_field_names(&decoded)
    } else {
        Vec::new()
    };
    let body = CapturedBody {
        kind,
        content_type: essence,
        byte_len: u32::try_from(decoded.len()).unwrap_or(u32::MAX),
        sha256: rd_authn::sha256_hex(&decoded),
        field_names,
        stored: refusal.is_none(),
    };
    let bytes = refusal.is_none().then_some(decoded);
    Ok((Some(body), bytes, refusal))
}

/// Field names of a form body, for the consent preview. Values are never extracted.
fn form_field_names(body: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for (name, _) in url::form_urlencoded::parse(text.as_bytes()) {
        let name = name.into_owned();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        if names.len() >= MAX_BODY_FIELD_NAMES {
            break;
        }
    }
    names
}

/// Trims a free-text field, dropping it when empty and rejecting it when oversized.
fn text_field(value: Option<String>, field: &str) -> Result<Option<String>, ApiError> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > MAX_CAPTURED_VALUE {
        return Err(field_length(field));
    }
    let value = value.trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

#[cfg(test)]
#[path = "capture_sanitize_tests.rs"]
mod tests;
