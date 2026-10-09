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
//!
//! An `.rdlinks` file (RD-1210-01) is rDownloader's own export: its links are proposals of a
//! document, held to the address rule of `LinkOrigin::Proposed`, and assigned to their hosts
//! again. Any container may be queued once its check has finished (`enqueue`). The NZBs such a
//! file carries (RD-1220-02) become NZB imports like a dropped NZB: in the LinkGrabber, or with
//! `enqueue` straight in the download list. A `.crawljob` is read by the host with the
//! `crawljob-intake` plugin's rules — no folder, no start of its own.

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use rd_api_core::input_checks::optional_text;
use rd_collector::ContainerFormat;

use crate::{
    ApiError, AppState,
    audit::AuditContext,
    collector_handlers::links::LinkOrigin,
    collector_source_sets::{from_own_hand, of_caller},
    container_upload::{ContainerUpload, UploadBody},
    dlc_import::{DlcImportOptions, DlcIntake},
};
use rd_api_core::{
    links_file::{Passphrase, read_links},
    links_nzb::NzbLanding,
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
    /// The NZBs an `.rdlinks` file carried, as NZB imports: in review, or already queued when
    /// `enqueue` was set (RD-1220-02). Empty for every other format.
    pub nzb_imports: Vec<rd_core::NzbImport>,
}

#[utoipa::path(post, path = "/api/v1/containers/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (ContainerUpload = "application/json"))), responses((status = 201, body = ContainerImportResponse), (status = 400, description = "The format is unknown, its import is disabled, the container is invalid, an encrypted link file's passphrase is missing or wrong, an NZB it carries is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit"), (status = 502, description = "The decryption service did not answer")))]
pub async fn import_container(
    State(state): State<AppState>,
    audit: AuditContext,
    body: UploadBody,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    import(state, &audit, body, None).await
}

/// The original DLC route, kept so an existing client and the browser extension keep working.
#[utoipa::path(post, path = "/api/v1/dlc/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (ContainerUpload = "application/json"))), responses((status = 201, body = ContainerImportResponse), (status = 400, description = "DLC import is disabled, the container is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit"), (status = 502, description = "The decryption service did not answer")))]
pub async fn import_dlc(
    State(state): State<AppState>,
    audit: AuditContext,
    body: UploadBody,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    import(state, &audit, body, Some(ContainerFormat::Dlc)).await
}

/// Both bodies arrive here as the same [`crate::container_upload::Upload`]; nothing below
/// knows which one it was.
async fn import(
    state: AppState,
    audit: &AuditContext,
    body: UploadBody,
    forced: Option<ContainerFormat>,
) -> Result<(StatusCode, Json<ContainerImportResponse>), ApiError> {
    let upload = body.read().await?;
    let enqueue = enqueue_flag(upload.enqueue.as_deref())?;
    let passphrase = Passphrase::given(upload.passphrase);
    let package_name = optional_text(upload.name);
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
    let options = DlcImportOptions {
        source: rd_core::IngressSource::Manual,
        source_label,
        fallback_name,
        fallback_password: password,
        category_id,
        priority,
    };
    // Followed before the import starts the check, so a check that is over at once is heard.
    let completed = enqueue.then(|| state.link_check.follow_completed());
    let outcome = if format == ContainerFormat::RdLinks {
        let document = read_links(&content, passphrase.as_ref()).await?;
        // A file a token hands in is a program's choice, never the person's own hand.
        let own_hand = from_own_hand(of_caller(audit, rd_core::IngressSource::Manual));
        let nzbs = if enqueue {
            NzbLanding::Enqueue(&state.scheduler)
        } else {
            NzbLanding::Review
        };
        crate::dlc_import::import_links(
            &intake,
            document,
            options,
            LinkOrigin::Proposed.reach(own_hand),
            nzbs,
        )
        .await?
    } else {
        let document = decode(&state, format, &content).await?;
        crate::dlc_import::import_document(&intake, document, options).await?
    };
    if let Some(completed) = completed {
        crate::container_enqueue::after_check(state.clone(), completed, &outcome.packages);
    }
    Ok((
        StatusCode::CREATED,
        Json(ContainerImportResponse {
            format: format.as_str().to_owned(),
            packages: outcome.packages,
            candidates: outcome.candidates,
            skipped_excluded: outcome.skipped_excluded,
            nzb_imports: outcome.nzb_imports,
        }),
    ))
}

/// The `enqueue` field: absent or empty is `false`, like an unticked box.
fn enqueue_flag(value: Option<&str>) -> Result<bool, ApiError> {
    match value.map(str::trim) {
        None | Some("" | "false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ApiError::bad_request(
            "container.enqueue_invalid",
            "enqueue must be true or false",
        )),
    }
}

/// Gets the links out, which is the only step that differs between the formats.
async fn decode(
    state: &AppState,
    format: ContainerFormat,
    content: &[u8],
) -> Result<rd_collector::DlcDocument, ApiError> {
    if format.needs_service() {
        let settings = crate::settings_store::read_settings(state).await?;
        return crate::dlc_import::decrypt_container(&settings, content, format.service_source())
            .await;
    }
    match format {
        ContainerFormat::Rsdf => rd_collector::decode_rsdf(content)
            .map_err(|error| ApiError::bad_request("container.file_invalid", format!("{error:#}"))),
        // Read by the host with the plugin's rules: no folder, no start of its own (RD-1220-02).
        ContainerFormat::CrawlJob => rd_collector::read_crawljob(content)
            .map_err(|error| ApiError::bad_request("container.file_invalid", format!("{error:#}"))),
        // A text list cannot fail to parse; it can only turn out to hold nothing, which
        // `import_document` reports as `dlc.no_links` like every other empty container.
        _ => Ok(rd_collector::parse_link_list(content)),
    }
}
