//! MCP tools for exporting packages as a link file and re-resolving downloads with the plugin
//! installed now (RD-1210-01).
//!
//! `export_packages` answers with the file itself, as text: an `.rdlinks` document (sealed when a
//! passphrase is given) or a `.crawljob`. The passphrase is the file's own, chosen for it, not a
//! stored credential; it goes into the key derivation and nowhere else, and the answer never
//! repeats it. The file goes back in through `import_container`. An `.rdlinks` file carries the
//! NZB documents of Usenet downloads and indexer hits (RD-1220-02); the ones the export could
//! not fetch are listed in `failed`, each with its coded reason.

use axum::{Json, extract::State};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, json_result, parse_ids, respond},
};
use crate::dto::{PackageExportFormat, PackageExportRequest, ReresolveRequest};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportFormatParam {
    /// rDownloader's own file: every package field and link detail, optionally encrypted.
    Rdlinks,
    /// JDownloader's plain-text file: addresses, package name and password only.
    Crawljob,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ExportPackagesParams {
    /// Download-list package ids (list_packages), with every file they hold.
    #[serde(default)]
    pub package_ids: Vec<String>,
    /// Single download ids (list_downloads).
    #[serde(default)]
    pub download_ids: Vec<String>,
    /// LinkGrabber package ids (list_collector), with every link not yet queued.
    #[serde(default)]
    pub collector_package_ids: Vec<String>,
    /// Every package of the download list.
    #[serde(default)]
    pub all: bool,
    pub format: ExportFormatParam,
    /// Encrypts an rdlinks file (at least 8 characters). Never echoed back; whoever imports
    /// the file needs it.
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReresolveDownloadsParams {
    /// Download ids (list_downloads).
    #[serde(default)]
    pub ids: Vec<String>,
    /// Package ids (list_packages): every file of each.
    #[serde(default)]
    pub package_ids: Vec<String>,
}

#[derive(Serialize)]
struct ExportedFileAnswer {
    file_name: String,
    content_type: &'static str,
    packages: usize,
    links: usize,
    nzbs: usize,
    skipped: usize,
    /// The NZBs among `skipped` the export could not fetch or no longer holds, with the reason.
    failed: Vec<crate::package_export::ExportFailure>,
    /// The file as text; hand it to import_container as base64 to take it back in.
    content: String,
}

#[tool_router(router = package_export_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Export packages as a link file: download-list packages, single downloads, LinkGrabber packages or `all`, as rdlinks (rDownloader's format: addresses, package name, password, category name, file names, sizes, checksums, mirror groups, and the NZB documents of Usenet downloads and indexer hits) or crawljob (JDownloader: addresses, package name, password; no NZBs). Never a plugin, plugin version, account, cookie or token, and never an indexer's address or API key: an indexer hit is fetched now and its NZB embedded, so import_container brings everything back on any installation, links assigned to their hosts again and NZBs imported like a dropped NZB. An NZB that cannot be fetched is skipped and listed in `failed` with its reason. A passphrase encrypts an rdlinks file, NZBs included, and is never echoed back. Answers with the file name, the counts and the file as text."
    )]
    pub async fn export_packages(
        &self,
        Parameters(params): Parameters<ExportPackagesParams>,
    ) -> McpToolResult {
        let request = match export_request(params) {
            Ok(request) => request,
            Err(error) => return Ok(api_error(error)),
        };
        let file = match crate::package_export::export_file(&self.state, request).await {
            Ok(file) => file,
            Err(error) => return Ok(api_error(error)),
        };
        json_result(&ExportedFileAnswer {
            file_name: file.file_name,
            content_type: file.content_type,
            packages: file.packages,
            links: file.links,
            nzbs: file.nzbs,
            skipped: file.skipped,
            failed: file.failed,
            content: String::from_utf8_lossy(&file.bytes).into_owned(),
        })
    }

    #[tool(
        description = "Resolve downloads again with the plugin version installed now: drops each download's binding to the plugin version that first resolved it (a download keeps that version for good otherwise, even after an update). A running file is paused and started again; finished files are left alone. Bytes already on disk are kept when the new resolution's size and ETag match, otherwise the file stops as blocked rather than mixing two files. By ids and/or package_ids, at most 500 files."
    )]
    pub async fn reresolve_downloads(
        &self,
        Parameters(params): Parameters<ReresolveDownloadsParams>,
    ) -> McpToolResult {
        let request = match parse_ids(&params.ids).and_then(|ids| {
            parse_ids(&params.package_ids).map(|package_ids| ReresolveRequest { ids, package_ids })
        }) {
            Ok(request) => request,
            Err(error) => return Ok(api_error(error)),
        };
        respond(
            crate::download_handlers::reresolve_downloads(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                Json(request),
            )
            .await
            .map(|answer| answer.0),
        )
    }
}

fn export_request(params: ExportPackagesParams) -> Result<PackageExportRequest, crate::ApiError> {
    Ok(PackageExportRequest {
        package_ids: parse_ids(&params.package_ids)?,
        download_ids: parse_ids(&params.download_ids)?,
        collector_package_ids: parse_ids(&params.collector_package_ids)?,
        all: params.all,
        format: match params.format {
            ExportFormatParam::Rdlinks => PackageExportFormat::Rdlinks,
            ExportFormatParam::Crawljob => PackageExportFormat::Crawljob,
        },
        passphrase: params
            .passphrase
            .map(rd_api_core::links_file::Passphrase::new),
    })
}
