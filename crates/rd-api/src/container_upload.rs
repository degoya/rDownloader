//! A container file, arriving either as a multipart upload or as base64 in a JSON body
//! (RD-120-31).
//!
//! ## One route, two bodies
//!
//! The four import routes — `containers/import`, `dlc/import`, `torrents/import` and
//! `nzb/imports` — each take a `multipart/form-data` upload, and a tool call or a script with a
//! JSON library has no convenient way to build one. They now take `application/json` as well,
//! **on the same route**, and [`UploadBody`] is the one place the two differ: it reads either
//! body into the same [`Upload`], and everything after that is the handler that was already
//! there. RD-120-29 found a second implementation answering from the manifest alone and
//! reporting every plugin as inactive; a second route for the same intake is how that starts,
//! so there is none. The same route also means the same entry in `scope_policy` — the JSON
//! form costs `api:intake` because it *is* the intake, not because a neighbour did.
//!
//! ## The size limit
//!
//! The whole request body is capped at [`BODY_LIMIT_BYTES`] (65 MiB), and base64 turns three
//! bytes into four. A JSON body may therefore carry a file of at most
//! [`MAX_JSON_CONTAINER_BYTES`], 48 MiB: its encoding is exactly 64 MiB, which leaves one MiB
//! for the rest of the body. A larger file is refused under `container.too_large` before it
//! is decoded, and a body over the service-wide limit is refused by [`code_oversized_json`]
//! under `request.body_too_large` — never truncated. The multipart form keeps its own limits:
//! an NZB of up to 64 MiB still arrives that way, and a torrent is capped at 16 MiB either way,
//! by the same check.

use axum::{
    Json,
    extract::{FromRequest, Multipart, Request, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::ApiError;

const MIB: usize = 1024 * 1024;

/// The service-wide request body limit, for every route.
///
/// Leaves room for multipart framing around the NZB handler's exact 64 MiB file limit.
pub(crate) const BODY_LIMIT_BYTES: usize = 65 * MIB;

/// The largest file a JSON body may carry. Its base64 is exactly 64 MiB; see the module text.
pub const MAX_JSON_CONTAINER_BYTES: usize = 48 * MIB;

/// Standard alphabet, padding optional: `base64 -w0` pads, some encoders do not, and a missing
/// `=` is not a reason to refuse a file.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// A container file handed in as JSON rather than as a multipart upload.
///
/// The fields are the multipart form's fields under the same names, and they are read by the
/// same code: `category_id` and `priority` stay strings for exactly that reason, so a value the
/// form refuses is refused here under the same code.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerUpload {
    /// The file's bytes, base64 with the standard alphabet. Padding is optional and line breaks
    /// are ignored. At most 48 MiB once decoded.
    pub content: String,
    /// The file's name, as a browser upload would carry it. `/api/v1/containers/import` reads
    /// the format from its extension, and a `{{password}}` marker in it names the archive
    /// password, as it does for a dropped file.
    #[serde(default)]
    pub file_name: Option<String>,
    /// The package name to use instead of the one the file suggests.
    #[serde(default)]
    pub name: Option<String>,
    /// The category the result is filed under.
    #[serde(default)]
    pub category_id: Option<String>,
    /// `low`, `normal` or `high`.
    #[serde(default)]
    pub priority: Option<String>,
    /// `dlc`, `ccf`, `rsdf` or `txt`, overriding the extension. Read by
    /// `/api/v1/containers/import` only; the other routes each take one format.
    #[serde(default)]
    pub format: Option<String>,
}

/// An import request's body, before it is read: a multipart upload or a JSON document.
pub enum UploadBody {
    Multipart(Multipart),
    Json(ContainerUpload),
}

/// What either body carried, in one shape.
#[derive(Debug, Default)]
pub(crate) struct Upload {
    pub name: Option<String>,
    pub category_id: Option<String>,
    pub priority: Option<String>,
    pub format: Option<String>,
    pub file: Option<UploadedFile>,
}

/// The `file` part of an upload.
#[derive(Debug)]
pub(crate) struct UploadedFile {
    /// The name the file arrived under, if it had one.
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

impl<S: Send + Sync> FromRequest<S> for UploadBody {
    /// A multipart body is refused exactly as the `Multipart` extractor always refused it; only
    /// a JSON body gets the codes below.
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if is_json(request.headers()) {
            return match Json::<ContainerUpload>::from_request(request, state).await {
                Ok(Json(upload)) => Ok(Self::Json(upload)),
                Err(rejection) => Err(json_rejection(&rejection).into_response()),
            };
        }
        Multipart::from_request(request, state)
            .await
            .map(Self::Multipart)
            .map_err(IntoResponse::into_response)
    }
}

impl UploadBody {
    /// Reads the body into an [`Upload`].
    ///
    /// Deferred rather than done in the extractor so a handler can refuse first — the torrent
    /// import checks its service switch before a single byte of the file is read.
    pub(crate) async fn read(self) -> Result<Upload, ApiError> {
        match self {
            Self::Multipart(multipart) => read_multipart(multipart).await,
            Self::Json(upload) => Ok(Upload {
                file: Some(UploadedFile {
                    bytes: decode_base64(&upload.content, MAX_JSON_CONTAINER_BYTES)?,
                    file_name: upload.file_name,
                }),
                name: upload.name,
                category_id: upload.category_id,
                priority: upload.priority,
                format: upload.format,
            }),
        }
    }
}

async fn read_multipart(mut multipart: Multipart) -> Result<Upload, ApiError> {
    let invalid = |error: axum::extract::multipart::MultipartError| {
        ApiError::bad_request("request.multipart_invalid", error.to_string())
    };
    let mut upload = Upload::default();
    while let Some(field) = multipart.next_field().await.map_err(invalid)? {
        let slot = match field.name() {
            Some("name") => &mut upload.name,
            Some("category_id") => &mut upload.category_id,
            Some("priority") => &mut upload.priority,
            Some("format") => &mut upload.format,
            Some("file") => {
                let file_name = field.file_name().map(str::to_owned);
                let bytes = field.bytes().await.map_err(invalid)?;
                upload.file = Some(UploadedFile {
                    file_name,
                    bytes: bytes.to_vec(),
                });
                continue;
            }
            _ => continue,
        };
        *slot = Some(field.text().await.map_err(invalid)?);
    }
    Ok(upload)
}

/// Decodes a base64 file, refusing it as too large before any of it is decoded.
///
/// Shared by the import routes and by a remote job's container, which has a smaller limit of
/// its own.
pub(crate) fn decode_base64(content: &str, max_bytes: usize) -> Result<Vec<u8>, ApiError> {
    let compact: String = content
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    if compact.len() > max_bytes.div_ceil(3) * 4 {
        return Err(too_large(max_bytes));
    }
    let bytes = BASE64.decode(compact.as_bytes()).map_err(|_| {
        ApiError::bad_request(
            "container.base64_invalid",
            "The content is not valid base64",
        )
    })?;
    // The bound above is rounded up to a whole group of four, which admits up to two bytes more.
    if bytes.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(bytes)
}

fn too_large(max_bytes: usize) -> ApiError {
    let max_mib = max_bytes / MIB;
    ApiError::payload_too_large(
        "container.too_large",
        format!("The file exceeds the {max_mib} MiB this request may carry"),
    )
    .with_param("max_mib", max_mib)
}

fn body_too_large() -> ApiError {
    let max_mib = BODY_LIMIT_BYTES / MIB;
    ApiError::payload_too_large(
        "request.body_too_large",
        format!("The request body exceeds the {max_mib} MiB the service accepts"),
    )
    .with_param("max_mib", max_mib)
}

/// Shared with `capture/file`, whose JSON body is a different document read the same way.
pub(crate) fn json_rejection(rejection: &JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return body_too_large();
    }
    ApiError::bad_request("request.body_invalid", rejection.body_text())
}

pub(crate) fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// Gives the service-wide body limit a stable code when a JSON request runs into it.
///
/// `RequestBodyLimitLayer` answers a declared length over the limit before any route runs, with
/// a bare `413` and English text. A JSON client — a tool call, a script — reads codes, and a
/// status with no code tells it nothing it can translate. Restricted to JSON requests on
/// purpose: what a browser's multipart upload is answered with is unchanged.
pub(crate) async fn code_oversized_json(request: Request, next: Next) -> Response {
    let json = is_json(request.headers());
    let response = next.run(request).await;
    if json && response.status() == StatusCode::PAYLOAD_TOO_LARGE && !is_json(response.headers()) {
        return body_too_large().into_response();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{MAX_JSON_CONTAINER_BYTES, decode_base64};

    #[test]
    fn padding_and_line_breaks_do_not_matter() {
        assert_eq!(decode_base64("aGVsbG8=", 16).expect("padded"), b"hello");
        assert_eq!(decode_base64("aGVsbG8", 16).expect("unpadded"), b"hello");
        assert_eq!(
            decode_base64("aGVs\nbG8=\n", 16).expect("wrapped"),
            b"hello"
        );
    }

    #[test]
    fn what_is_not_base64_says_so() {
        let error = decode_base64("not base64!", 16).expect_err("refused");
        assert_eq!(error.code(), "container.base64_invalid");
    }

    /// The limit is the decoded size, exactly: the rounding of the cheap bound must not let a
    /// file one byte over through.
    #[test]
    fn the_limit_is_exact() {
        use base64::Engine;
        let at = base64::engine::general_purpose::STANDARD.encode(vec![0_u8; 16]);
        assert_eq!(decode_base64(&at, 16).expect("at the limit").len(), 16);
        let over = base64::engine::general_purpose::STANDARD.encode(vec![0_u8; 17]);
        let error = decode_base64(&over, 16).expect_err("one byte over");
        assert_eq!(error.code(), "container.too_large");
    }

    /// 48 MiB is the file whose base64 is exactly 64 MiB, which is what fits under the body limit.
    #[test]
    fn the_json_limit_encodes_to_what_the_body_limit_leaves_room_for() {
        assert_eq!(MAX_JSON_CONTAINER_BYTES.div_ceil(3) * 4, 64 * 1024 * 1024);
        assert!(MAX_JSON_CONTAINER_BYTES.div_ceil(3) * 4 < super::BODY_LIMIT_BYTES);
    }
}
