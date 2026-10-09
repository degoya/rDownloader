//! Exporting packages as a link file (RD-1210-01).
//!
//! Download-list packages, single downloads and LinkGrabber packages leave as one `.rdlinks`
//! document — or a `.crawljob` for JDownloader — carrying what makes the links and nothing that
//! binds them to this installation: the address a person gave (never the direct address a
//! resolver answered with, never a user name or password inside it), the package's name,
//! archive password and category name, and per link its file name, size, checksum and mirror
//! group. No plugin, version, account, cookie or token, so a file brought back in is assigned to
//! its host again and resolved by the plugin installed then. Finished, running and failed files
//! are exported alike. A Usenet file has no address of its own and a link with a scheme the
//! format does not carry is counted as skipped rather than written half.

use std::collections::HashMap;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use rd_collector::{LinksDocument, LinksEntry, LinksPackage};
use rd_core::{DownloadFile, DownloadKind, PackageId};

use crate::{
    ApiError, AppState,
    dto::{PackageExportFormat, PackageExportRequest},
};
use rd_api_core::links_file::{Passphrase, seal_links};

/// A written export, ready to hand out.
pub struct ExportedFile {
    pub file_name: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
    pub packages: usize,
    pub links: usize,
    /// Links the format could not carry: a Usenet file, a local file, a scheme it lacks.
    pub skipped: usize,
}

#[utoipa::path(
    post,
    path = "/api/v1/packages/export",
    tag = "downloads",
    request_body = PackageExportRequest,
    responses(
        (status = 200, description = "The file, as an attachment", content((String = "application/json"), (String = "text/plain")), headers(("x-rd-export-links" = u32, description = "Links in the file"), ("x-rd-export-skipped" = u32, description = "Links the format could not carry"))),
        (status = 400, description = "Nothing selected or nothing exportable, too many links, a passphrase too short, or a passphrase for a crawljob"),
        (status = 404, description = "A named package or download does not exist")
    )
)]
pub async fn export_packages(
    State(state): State<AppState>,
    Json(request): Json<PackageExportRequest>,
) -> Result<Response, ApiError> {
    let file = export_file(&state, request).await?;
    let disposition =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file.file_name))
            .map_err(anyhow::Error::new)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(file.content_type),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::HeaderName::from_static("x-rd-export-links"),
                HeaderValue::from(file.links),
            ),
            (
                header::HeaderName::from_static("x-rd-export-skipped"),
                HeaderValue::from(file.skipped),
            ),
        ],
        Body::from(file.bytes),
    )
        .into_response())
}

/// Collects the selection and writes it in the requested format; the MCP tool answers with it.
///
/// # Errors
///
/// `export.nothing_selected`, `export.nothing_exportable`, `export.passphrase_unsupported`,
/// `rdlinks.links_limit` and `rdlinks.passphrase_too_short` (`400`); a package or download
/// that does not exist (`404`).
pub async fn export_file(
    state: &AppState,
    request: PackageExportRequest,
) -> Result<ExportedFile, ApiError> {
    let passphrase = Passphrase::given(request.passphrase.clone());
    if request.format == PackageExportFormat::Crawljob && passphrase.is_some() {
        return Err(ApiError::bad_request(
            "export.passphrase_unsupported",
            "A crawljob cannot be encrypted; export an rdlinks file to use a passphrase",
        ));
    }
    let (document, mut skipped) = collect(state, &request).await?;
    let links = rd_collector::link_count(&document);
    if links == 0 {
        return Err(ApiError::bad_request(
            "export.nothing_exportable",
            "None of the selected links can be written to a link file",
        ));
    }
    if links > rd_collector::MAX_RDLINKS_LINKS {
        return Err(rd_api_core::dlc_import::links_limit());
    }
    let packages = document.packages.len();
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let (extension, content_type, bytes) = match request.format {
        PackageExportFormat::Rdlinks => {
            let bytes = match &passphrase {
                Some(passphrase) => seal_links(&document, passphrase).await?,
                None => rd_collector::write_links_file(&document)?,
            };
            ("rdlinks", "application/json", bytes)
        }
        PackageExportFormat::Crawljob => {
            let written = rd_collector::write_crawljob(&document);
            skipped += written.skipped;
            (
                "crawljob",
                "text/plain; charset=utf-8",
                written.text.into_bytes(),
            )
        }
    };
    Ok(ExportedFile {
        file_name: format!("rdownloader-{stamp}.{extension}"),
        content_type,
        bytes,
        packages,
        links,
        skipped,
    })
}

/// The selected packages as a document, and how many links could not go into it.
async fn collect(
    state: &AppState,
    request: &PackageExportRequest,
) -> Result<(LinksDocument, usize), ApiError> {
    let named = request.package_ids.len()
        + request.download_ids.len()
        + request.collector_package_ids.len();
    if named == 0 && !request.all {
        return Err(ApiError::bad_request(
            "export.nothing_selected",
            "Select a package, a download or a LinkGrabber package to export",
        ));
    }
    if named > rd_collector::MAX_RDLINKS_LINKS {
        return Err(rd_api_core::dlc_import::links_limit());
    }
    let categories: HashMap<rd_core::CategoryId, String> = state
        .database
        .list_categories()
        .await?
        .into_iter()
        .map(|category| (category.id, category.name))
        .collect();
    let mut export = Export {
        document: LinksDocument::default(),
        skipped: 0,
        categories,
    };
    for (package_id, only) in queue_selection(state, request).await? {
        export.add_queue_package(state, package_id, only).await?;
    }
    for &package_id in &request.collector_package_ids {
        export.add_collector_package(state, package_id).await?;
    }
    Ok((export.document, export.skipped))
}

/// Download-list packages in the order they were named, each with the files to take: `None`
/// for all of them.
async fn queue_selection(
    state: &AppState,
    request: &PackageExportRequest,
) -> Result<Vec<(PackageId, Option<Vec<DownloadFile>>)>, ApiError> {
    let mut whole: Vec<PackageId> = if request.all {
        state
            .database
            .list_packages()
            .await?
            .into_iter()
            .map(|package| package.id)
            .collect()
    } else {
        Vec::new()
    };
    for &id in &request.package_ids {
        if !whole.contains(&id) {
            whole.push(id);
        }
    }
    let mut selection: Vec<(PackageId, Option<Vec<DownloadFile>>)> =
        whole.into_iter().map(|id| (id, None)).collect();
    for &id in &request.download_ids {
        let file = state
            .database
            .get_download(id)
            .await?
            .ok_or_else(crate::error_codes::download_not_found)?;
        match selection
            .iter_mut()
            .find(|(package, _)| *package == file.package_id)
        {
            // The whole package is exported already, or this file joins the others of it.
            Some((_, None)) => {}
            Some((_, Some(files))) => {
                if !files.iter().any(|known| known.id == file.id) {
                    files.push(file);
                }
            }
            None => selection.push((file.package_id, Some(vec![file]))),
        }
    }
    Ok(selection)
}

struct Export {
    document: LinksDocument,
    skipped: usize,
    categories: HashMap<rd_core::CategoryId, String>,
}

impl Export {
    async fn add_queue_package(
        &mut self,
        state: &AppState,
        package_id: PackageId,
        only: Option<Vec<DownloadFile>>,
    ) -> Result<(), ApiError> {
        let package = state
            .database
            .get_package(package_id)
            .await?
            .ok_or_else(crate::error_codes::package_not_found)?;
        let files = match only {
            Some(files) => files,
            None => state.database.downloads_for_package(package_id).await?,
        };
        let mut links = Vec::with_capacity(files.len());
        for file in files {
            match queue_link(&file) {
                Some(link) => links.push(link),
                None => self.skipped += 1,
            }
        }
        let password = if package.has_password {
            state.database.package_password(package_id).await?
        } else {
            None
        };
        self.push(package.name, password, package.category_id, links);
        Ok(())
    }

    async fn add_collector_package(
        &mut self,
        state: &AppState,
        package_id: rd_core::CollectorPackageId,
    ) -> Result<(), ApiError> {
        let package = state
            .database
            .get_collector_package(package_id)
            .await?
            .ok_or_else(crate::error_codes::package_not_found)?;
        let mut candidates: Vec<rd_core::LinkCandidate> = state
            .database
            .list_candidates()
            .await?
            .into_iter()
            .filter(|candidate| {
                candidate.package_id == Some(package_id)
                    && candidate.state != rd_core::LinkCandidateState::Enqueued
            })
            .collect();
        candidates.sort_by_key(|candidate| candidate.position);
        let mut links = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if !rd_collector::carries_scheme(&candidate.url) {
                self.skipped += 1;
                continue;
            }
            links.push(LinksEntry {
                url: bare(&candidate.url),
                file_name: candidate.file_name.filter(|_| candidate.file_name_declared),
                size: candidate.size.map(rd_core::ByteCount::get),
                checksum: None,
                mirror_group: candidate.mirror.map(|mirror| mirror.group),
            });
        }
        self.push(package.name, package.password, package.category_id, links);
        Ok(())
    }

    fn push(
        &mut self,
        name: String,
        password: Option<String>,
        category_id: Option<rd_core::CategoryId>,
        links: Vec<LinksEntry>,
    ) {
        if links.is_empty() {
            return;
        }
        self.document.packages.push(LinksPackage {
            name: Some(name),
            password: password.filter(|password| !password.is_empty()),
            category: category_id.and_then(|id| self.categories.get(&id).cloned()),
            comment: None,
            links,
        });
    }
}

/// One queued file as a link, or `None` when the format cannot carry it.
fn queue_link(file: &DownloadFile) -> Option<LinksEntry> {
    if file.kind == DownloadKind::Usenet || !rd_collector::carries_scheme(&file.source) {
        return None;
    }
    Some(LinksEntry {
        url: bare(&file.source),
        file_name: Some(file.file_name.clone()).filter(|name| !name.is_empty()),
        size: file.total_bytes.map(rd_core::ByteCount::get),
        checksum: file.expected_checksum.clone(),
        mirror_group: file.mirror_group.clone(),
    })
}

/// The address without a user name, a password or a query value that carries a credential
/// (`apikey`, `token`, `passkey`, … — `rd_core::is_secret_parameter`): a login stays in this
/// installation's vault, and an indexer's key in its download address must not travel in a
/// file. An address with nothing to hide comes out byte for byte.
fn bare(url: &url::Url) -> url::Url {
    let mut url = url.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if url
        .query_pairs()
        .any(|(name, _)| rd_core::is_secret_parameter(&name))
    {
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(name, _)| !rd_core::is_secret_parameter(name))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            url.set_query(None);
        } else {
            url.query_pairs_mut().clear().extend_pairs(kept);
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::bare;

    #[test]
    fn a_login_or_a_key_inside_an_address_stays_behind() {
        let address: url::Url = "ftp://user:secret@example.com/file.bin"
            .parse()
            .expect("address");
        assert_eq!(bare(&address).as_str(), "ftp://example.com/file.bin");
        let plain: url::Url = "https://ddownload.com/abc?x=1".parse().expect("address");
        assert_eq!(bare(&plain), plain);
        let indexer: url::Url = "https://indexer.example/getnzb?id=5&apikey=secret&r=abc"
            .parse()
            .expect("address");
        assert_eq!(
            bare(&indexer).as_str(),
            "https://indexer.example/getnzb?id=5&r=abc"
        );
        let only: url::Url = "https://cdn.example/f.bin?token=abc"
            .parse()
            .expect("address");
        assert_eq!(bare(&only).as_str(), "https://cdn.example/f.bin");
    }
}
