//! Uploading a container to the LinkGrabber.
//!
//! One endpoint for every format, because the only thing that differs between them is how the
//! links are got out: everything after that — the domain blocklist, the credential vault, the
//! package naming, the online check — is the same work. The format comes from the file's
//! extension, with an explicit field to override it.
//!
//! The file arrives as a multipart upload or, for a caller without one, as base64 in a JSON
//! body (RD-120-31); [`crate::container_upload`] reads both into one shape before any of this
//! runs.

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use rd_collector::ContainerFormat;

use crate::{
    ApiError, AppState,
    container_upload::{ContainerUpload, UploadBody},
    dlc_import::{DlcImportOptions, DlcIntake},
};

/// A container may hold several packages, so the import answers with all of them at once.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContainerImportResponse {
    /// Which format the upload turned out to be.
    pub format: String,
    pub packages: Vec<rd_core::CollectorPackage>,
    pub candidates: Vec<rd_core::LinkCandidate>,
    /// Links the domain blocklist dropped before they reached the LinkGrabber.
    pub skipped_excluded: u32,
}

#[utoipa::path(post, path = "/api/v1/containers/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (ContainerUpload = "application/json"))), responses((status = 201, body = ContainerImportResponse), (status = 400, description = "The format is unknown, its import is disabled, the container is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit"), (status = 502, description = "The decryption service did not answer")))]
pub async fn import_container(
    State(state): State<AppState>,
    body: UploadBody,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    import(state, body, None).await
}

/// The original DLC route, kept so an existing client and the browser extension keep working.
#[utoipa::path(post, path = "/api/v1/dlc/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (ContainerUpload = "application/json"))), responses((status = 201, body = ContainerImportResponse), (status = 400, description = "DLC import is disabled, the container is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit"), (status = 502, description = "The decryption service did not answer")))]
pub async fn import_dlc(
    State(state): State<AppState>,
    body: UploadBody,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    import(state, body, Some(ContainerFormat::Dlc)).await
}

/// Both bodies arrive here as the same [`crate::container_upload::Upload`]; nothing below
/// knows which one it was.
async fn import(
    state: AppState,
    body: UploadBody,
    forced: Option<ContainerFormat>,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    let upload = body.read().await?;
    let package_name = upload
        .name
        .map(|value| value.trim().to_owned())
        .filter(|name| !name.is_empty());
    let category_id = match upload.category_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(id.parse::<rd_core::CategoryId>().map_err(|_| {
            ApiError::bad_request("dlc.category_invalid", "Category id is not valid")
        })?),
    };
    let priority = match upload.priority.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("low") => Some(rd_core::DownloadPriority::Low),
        Some("normal") => Some(rd_core::DownloadPriority::Normal),
        Some("high") => Some(rd_core::DownloadPriority::High),
        Some(_) => {
            return Err(ApiError::bad_request(
                "dlc.priority_invalid",
                "Priority must be low, normal or high",
            ));
        }
    };
    let declared = match upload.format.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(trimmed) => Some(
            ContainerFormat::from_file_name(&format!("x.{trimmed}")).ok_or_else(|| {
                ApiError::bad_request(
                    "container.format_unknown",
                    format!("{trimmed} is not a container format this build reads"),
                )
            })?,
        ),
    };
    let Some(file) = upload.file else {
        return Err(ApiError::bad_request(
            "request.multipart_missing_file",
            "Multipart field 'file' is missing",
        ));
    };
    let content = file.bytes;
    let source_label = file.file_name;
    // Same convention as an NZB: `release{{password}}.dlc` carries the archive password.
    let (stem, password) =
        rd_files::strip_password_marker(source_label.as_deref().unwrap_or("import.dlc"));
    let format = forced
        .or(declared)
        .or_else(|| ContainerFormat::from_file_name(&stem))
        .ok_or_else(|| {
            ApiError::bad_request(
                "container.format_unknown",
                "The file name does not name a container format this build reads",
            )
        })?;
    let document = decode(&state, format, &content).await?;
    let fallback_name = package_name.or_else(|| {
        let stem = stem
            .rsplit_once('.')
            .map_or(stem.as_str(), |(base, _)| base);
        let stem = rd_files::sanitize_file_name(stem.trim());
        (!stem.is_empty()).then_some(stem)
    });
    let intake = DlcIntake {
        database: &state.database,
        secrets: &state.secrets,
        link_check: &state.link_check,
        media: state.media_settings.read().await.clone(),
        gallery: state.gallery_settings.read().await.clone(),
    };
    let outcome = crate::dlc_import::import_document(
        &intake,
        document,
        DlcImportOptions {
            source: rd_core::IngressSource::Manual,
            source_label,
            fallback_name,
            fallback_password: password,
            category_id,
            priority,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ContainerImportResponse {
            format: format.as_str().to_owned(),
            packages: outcome.packages,
            candidates: outcome.candidates,
            skipped_excluded: outcome.skipped_excluded,
        }),
    ))
}

/// Gets the links out, which is the only step that differs between the formats.
async fn decode(
    state: &AppState,
    format: ContainerFormat,
    content: &[u8],
) -> Result<rd_collector::DlcDocument, ApiError> {
    if format.needs_service() {
        let settings = crate::handlers::read_settings(state).await?;
        return crate::dlc_import::decrypt_container(&settings, content, format.service_source())
            .await;
    }
    match format {
        ContainerFormat::Rsdf => rd_collector::decode_rsdf(content)
            .map_err(|error| ApiError::bad_request("container.file_invalid", format!("{error:#}"))),
        // A text list cannot fail to parse; it can only turn out to hold nothing, which
        // `import_document` reports as `dlc.no_links` like every other empty container.
        _ => Ok(rd_collector::parse_link_list(content)),
    }
}
