//! `POST /api/v1/capture/file`: a file only the browser could load, handed over (RD-130-16).
//!
//! An indexer's cart, a one-time link, a download behind a session: rDownloader fetching the
//! address again, without the browser's session, gets an error page or nothing (RD-120-63). So
//! the browser extension hands over what it has instead, after the person allowed the site:
//!
//! - **the bytes** the browser received — Firefox copies a response with `filterResponseData` —
//!   as a `multipart/form-data` upload (`file`, and optionally `file_name`), or as base64 in
//!   `content` of a JSON body; or
//! - **the address and that host's cookies** as JSON (`url`, `cookies`, `referrer`,
//!   `user_agent`), where the browser cannot copy a response. [`crate::capture_fetch`] fetches
//!   it exactly once and forgets the cookies.
//!
//! Either way the bytes go straight to the importer, and nothing fetches the file again. What
//! is understood: an NZB, a `.torrent`, and a ZIP of NZBs as an NNTmux cart hands it out. Any
//! other file is refused under `capture.file_unsupported`, and the extension leaves it to the
//! browser — which is where it would have stayed without this route.

use axum::{
    Json,
    extract::{FromRequest, Multipart, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    capture_fetch::{self, FetchPlan},
    container_upload::{MAX_JSON_CONTAINER_BYTES, decode_base64, is_json, json_rejection},
    dto::CollectorIntakeResponse,
    nzb_zip::{self, ZipLimits},
};

/// The largest file a hand-over may carry: the limit an uploaded NZB has.
pub(crate) const MAX_CAPTURE_FILE_BYTES: usize = rd_collector::MAX_NZB_BYTES;

/// A file handed over as JSON: either its bytes or its address, never both.
///
/// No `Debug`: the cookies are the one thing in this service's request bodies that must never
/// reach a log line, and a derived `Debug` is how a body ends up in one.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureFileRequest {
    /// The file's bytes as the browser received them, base64 with the standard alphabet. At
    /// most 48 MiB once decoded.
    #[serde(default)]
    pub content: Option<String>,
    /// The address to fetch once, when the browser could not copy the bytes. http or https.
    #[serde(default)]
    pub url: Option<String>,
    /// The cookies the browser would send to `url`, in the form `capture/cookies` takes: a
    /// Netscape cookie file, or a `Cookie` header. A cookie the browser would not send to `url`
    /// refuses the request. They travel only to that address's own scheme, host and port, and
    /// are never stored. Only with `url`.
    #[serde(default)]
    #[schema(write_only)]
    pub cookies: Option<String>,
    /// The page the download started from, sent as `Referer`. Only with `url`.
    #[serde(default)]
    pub referrer: Option<String>,
    /// The browser's user agent, which some sites bind a session to. Only with `url`.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// The file's name, as the browser would save it.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// What the file was taken as.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFileKind {
    Nzb,
    Torrent,
    NzbZip,
}

/// What a hand-over became.
#[derive(Serialize, ToSchema)]
pub struct CaptureFileResponse {
    pub kind: CaptureFileKind,
    /// The NZB imports, one per NZB; empty for a torrent.
    pub nzb_imports: Vec<rd_core::NzbImport>,
    /// The LinkGrabber intake a torrent became; absent otherwise.
    pub torrent: Option<CollectorIntakeResponse>,
}

/// The request body before it is read: an upload, or a JSON document.
pub enum CaptureFileBody {
    Multipart(Multipart),
    Json(CaptureFileRequest),
}

impl<S: Send + Sync> FromRequest<S> for CaptureFileBody {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if is_json(request.headers()) {
            return match Json::<CaptureFileRequest>::from_request(request, state).await {
                Ok(Json(body)) => Ok(Self::Json(body)),
                Err(rejection) => Err(json_rejection(&rejection).into_response()),
            };
        }
        Multipart::from_request(request, state)
            .await
            .map(Self::Multipart)
            .map_err(IntoResponse::into_response)
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/capture/file",
    tag = "capture",
    request_body(content((Vec<u8> = "multipart/form-data"), (CaptureFileRequest = "application/json"))),
    responses(
        (status = 201, body = CaptureFileResponse),
        (status = 400, description = "Not an NZB, a torrent or a ZIP of NZBs; a field, a cookie or a header is invalid; or both or neither of content and url were sent"),
        (status = 401, description = "Capture token missing or revoked"),
        (status = 413, description = "The file exceeds 64 MiB (48 MiB as base64 JSON)"),
        (status = 502, description = "The address could not be fetched")
    )
)]
pub async fn capture_file(
    State(state): State<AppState>,
    body: CaptureFileBody,
) -> Result<(StatusCode, Json<CaptureFileResponse>), ApiError> {
    let (bytes, file_name) = match body {
        CaptureFileBody::Multipart(multipart) => read_multipart(multipart).await?,
        CaptureFileBody::Json(request) => read_json(request).await?,
    };
    let response = import(&state, bytes, file_name).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn read_multipart(mut multipart: Multipart) -> Result<(Vec<u8>, Option<String>), ApiError> {
    let invalid = |error: axum::extract::multipart::MultipartError| {
        ApiError::bad_request("request.multipart_invalid", error.to_string())
    };
    let mut file = None;
    let mut named = None;
    while let Some(field) = multipart.next_field().await.map_err(invalid)? {
        match field.name() {
            Some("file") => {
                let uploaded = field.file_name().map(str::to_owned);
                let bytes = field.bytes().await.map_err(invalid)?;
                file = Some((bytes.to_vec(), uploaded));
            }
            Some("file_name") => named = Some(field.text().await.map_err(invalid)?),
            _ => {}
        }
    }
    let (bytes, uploaded) = file.ok_or_else(|| {
        ApiError::bad_request(
            "request.multipart_missing_file",
            "Multipart field 'file' is missing",
        )
    })?;
    if bytes.len() > MAX_CAPTURE_FILE_BYTES {
        return Err(capture_fetch::file_too_large(MAX_CAPTURE_FILE_BYTES));
    }
    Ok((bytes, named.or(uploaded)))
}

async fn read_json(request: CaptureFileRequest) -> Result<(Vec<u8>, Option<String>), ApiError> {
    let fetch_only =
        request.cookies.is_some() || request.referrer.is_some() || request.user_agent.is_some();
    match (request.content, request.url) {
        (Some(content), None) if !fetch_only => Ok((
            decode_base64(&content, MAX_JSON_CONTAINER_BYTES)?,
            request.file_name,
        )),
        (None, Some(url)) => {
            let plan = FetchPlan::new(
                &url,
                request.cookies.as_deref(),
                request.referrer.as_deref(),
                request.user_agent.as_deref(),
            )?;
            // The request's cookies are dropped here, with the body they came in.
            drop(request.cookies);
            let fetched = capture_fetch::fetch_once(&plan, MAX_CAPTURE_FILE_BYTES).await?;
            Ok((fetched.bytes, request.file_name.or(fetched.file_name)))
        }
        _ => Err(ApiError::bad_request(
            "capture.file_source_invalid",
            "Send either the file's content or its address with its cookies, not both",
        )),
    }
}

/// Takes the bytes as whatever they are, by content rather than by name.
async fn import(
    state: &AppState,
    bytes: Vec<u8>,
    file_name: Option<String>,
) -> Result<CaptureFileResponse, ApiError> {
    let source = rd_core::IngressSource::BrowserDownload;
    if nzb_zip::is_zip(&bytes) {
        let members = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
            let members = nzb_zip::nzb_members(&bytes, ZipLimits::DEFAULT)?;
            // Every member is parsed before the first is stored: a cart is imported whole or
            // not at all, and the browser keeps its copy in the second case.
            for member in &members {
                rd_collector::parse_nzb(&member.bytes).map_err(|error| {
                    ApiError::bad_request("nzb.parse_failed", error.to_string())
                        .with_param("file_name", &member.file_name)
                })?;
            }
            Ok(members)
        })
        .await
        .map_err(anyhow::Error::new)??;
        let mut nzb_imports = Vec::with_capacity(members.len());
        for member in members {
            nzb_imports.push(
                crate::handlers::store_nzb_import(
                    state,
                    &member.bytes,
                    &member.file_name,
                    None,
                    source,
                    None,
                )
                .await?,
            );
        }
        return Ok(CaptureFileResponse {
            kind: CaptureFileKind::NzbZip,
            nzb_imports,
            torrent: None,
        });
    }
    if looks_like_torrent(&bytes) {
        crate::torrent_handlers::ensure_torrent_service_enabled(state).await?;
        if bytes.len() > rd_torrent::MAX_TORRENT_BYTES {
            return Err(ApiError::bad_request(
                "torrent.file_too_large",
                "Torrent file exceeds the 16 MiB limit",
            ));
        }
        rd_torrent::parse_torrent(&bytes)
            .map_err(|error| ApiError::bad_request("torrent.file_invalid", format!("{error:#}")))?;
        let (batch, packages, candidates) = crate::torrent_handlers::add_torrent_to_collector(
            &state.database,
            &state.torrent,
            &bytes,
            source,
            file_name,
            None,
            None,
            None,
        )
        .await?;
        return Ok(CaptureFileResponse {
            kind: CaptureFileKind::Torrent,
            nzb_imports: Vec::new(),
            torrent: Some(CollectorIntakeResponse {
                batch,
                packages,
                candidates,
                skipped_excluded: 0,
                skipped_disabled: 0,
                crawled_found: 0,
                crawled_dropped: 0,
            }),
        });
    }
    if looks_like_nzb(&bytes) {
        let name = file_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "browser.nzb".to_owned());
        let import =
            crate::handlers::store_nzb_import(state, &bytes, &name, None, source, None).await?;
        return Ok(CaptureFileResponse {
            kind: CaptureFileKind::Nzb,
            nzb_imports: vec![import],
            torrent: None,
        });
    }
    Err(ApiError::bad_request(
        "capture.file_unsupported",
        "The file is not an NZB, a torrent or a ZIP of NZBs",
    ))
}

/// A bencoded dictionary with an `info` key: what every `.torrent` is.
fn looks_like_torrent(bytes: &[u8]) -> bool {
    bytes.first() == Some(&b'd') && contains(bytes, b"4:info")
}

/// An `<nzb` element near the start, after any XML declaration and doctype. An HTML login page
/// an indexer answers an expired session with is not one.
fn looks_like_nzb(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    contains(&head.to_ascii_lowercase(), b"<nzb")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{looks_like_nzb, looks_like_torrent};

    #[test]
    fn the_kind_is_read_from_the_content_not_the_name() {
        assert!(looks_like_nzb(
            b"<?xml version=\"1.0\"?>\n<!DOCTYPE nzb>\n<nzb xmlns=\"x\"></nzb>"
        ));
        assert!(!looks_like_nzb(b"<!doctype html><title>Login</title>"));
        assert!(looks_like_torrent(b"d8:announce3:foo4:infod4:name1:xee"));
        assert!(!looks_like_torrent(b"<nzb/>"));
        assert!(!looks_like_torrent(b"d3:foo3:bare"));
    }
}
