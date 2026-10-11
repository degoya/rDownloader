//! The NZBs of a package export (RD-1220-02).
//!
//! A queue package from an NZB keeps its import's files, groups and articles, not the document
//! it came from, so its NZB is written out from them (`rd_collector::render_nzb`), as a remote
//! job hands one over. An indexer hit still in the LinkGrabber is fetched exactly as its enqueue
//! fetches it — `rd_api_core::nzb_candidate`, with the candidate's address rule and the
//! indexer's refusal — and written out the same way, so the file holds the release's articles
//! and never the indexer's address or key. Either one that cannot be had is skipped and named
//! with its reason; one NZB is never worth the whole export.

use std::{collections::HashSet, fmt::Write as _};

use chrono::Utc;
use rd_collector::LinksNzb;
use rd_core::{DownloadFile, DownloadPackage, LinkCandidate};
use serde::Serialize;

use super::Export;
use crate::{ApiError, AppState, dto::MessageResponse};

/// The most failures the HTTP answer names in its header; the count covers the rest.
const HEADER_FAILURES: usize = 10;

/// An NZB the export could not carry, and why.
#[derive(Serialize)]
pub struct ExportFailure {
    /// The package or release it belonged to.
    pub name: String,
    #[serde(flatten)]
    pub reason: MessageResponse,
}

impl Export {
    /// The NZB behind a queue package's Usenet files, narrowed to the files selected.
    pub(super) async fn queue_nzbs(
        &mut self,
        state: &AppState,
        package: &DownloadPackage,
        files: &[DownloadFile],
    ) -> Result<Vec<LinksNzb>, ApiError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        if !self.embed_nzbs {
            self.skipped += files.len();
            return Ok(Vec::new());
        }
        match queue_nzb(state, package, files).await? {
            Ok(nzb) => Ok(vec![nzb]),
            Err(reason) => {
                self.skipped += files.len();
                self.fail(&package.name, reason);
                Ok(Vec::new())
            }
        }
    }

    /// The NZB behind an indexer hit of the LinkGrabber, or `None` when it is skipped.
    pub(super) async fn collector_nzb(
        &mut self,
        state: &AppState,
        candidate: &LinkCandidate,
        package: &rd_core::CollectorPackage,
    ) -> Option<LinksNzb> {
        if !self.embed_nzbs {
            self.skipped += 1;
            return None;
        }
        match collector_nzb(state, candidate, package).await {
            Ok(nzb) => Some(nzb),
            Err(error) => {
                self.skipped += 1;
                let name = candidate
                    .file_name
                    .clone()
                    .filter(|_| candidate.file_name_declared)
                    .unwrap_or_else(|| package.name.clone());
                tracing::info!(
                    candidate = %candidate.id,
                    code = error.code(),
                    "an NZB was left out of an export"
                );
                self.fail(&name, error);
                None
            }
        }
    }

    fn fail(&mut self, name: &str, error: ApiError) {
        self.failed.push(ExportFailure {
            name: name.chars().take(120).collect(),
            reason: error.into_message(),
        });
    }
}

/// The stored NZB of `package`, or the refusal to name when it has none any more.
async fn queue_nzb(
    state: &AppState,
    package: &DownloadPackage,
    files: &[DownloadFile],
) -> Result<Result<LinksNzb, ApiError>, ApiError> {
    let import = match package.nzb_import_id {
        Some(id) => state.database.get_nzb_import(id).await?,
        None => None,
    };
    let Some(import) = import else {
        return Ok(Err(unavailable()));
    };
    let wanted: HashSet<rd_core::NzbFileId> =
        files.iter().filter_map(|file| file.nzb_file_id).collect();
    let stored: Vec<rd_core::NzbFileStatus> = state
        .database
        .list_nzb_files(import.id)
        .await?
        .into_iter()
        .filter(|file| wanted.is_empty() || wanted.contains(&file.id))
        .collect();
    if stored.is_empty() {
        return Ok(Err(unavailable()));
    }
    let name = rd_collector::container_name(&import.name);
    let document = crate::nzb_remote_job_handlers::document(import.password.clone(), stored);
    Ok(written(&document, name, import.created_at.timestamp()))
}

/// An indexer hit's NZB, fetched as the enqueue fetches it, with the password it would take.
async fn collector_nzb(
    state: &AppState,
    candidate: &LinkCandidate,
    package: &rd_core::CollectorPackage,
) -> Result<LinksNzb, ApiError> {
    let reach = state.database.candidate_remote_reach(candidate.id).await?;
    let fetched = rd_api_core::nzb_candidate::fetch_nzb_candidate(state, candidate, reach).await?;
    let release = fetched.release(state, candidate, package).await?;
    let mut document = fetched.document;
    // The password the enqueue would have taken travels inside the NZB, so the import takes it
    // in the same order.
    document.password = release.password;
    // Each file keeps the post date the indexer's NZB announced; the time of the export only
    // stands in for a file that announced none (RD-1240-33).
    written(&document, release.name, Utc::now().timestamp())
}

/// `document` as the XML the file carries.
fn written(
    document: &rd_collector::NzbDocument,
    name: String,
    date: i64,
) -> Result<LinksNzb, ApiError> {
    let content = rd_collector::render_nzb(document, Some(&name), date);
    if content.len() > rd_collector::MAX_NZB_BYTES {
        let max_mib = rd_collector::MAX_NZB_BYTES >> 20;
        return Err(ApiError::bad_request(
            "export.nzb_too_large",
            format!("The NZB exceeds the {max_mib} MiB a link file carries per NZB"),
        )
        .with_param("max_mib", max_mib));
    }
    let content = String::from_utf8(content).map_err(anyhow::Error::new)?;
    Ok(LinksNzb { name, content })
}

fn unavailable() -> ApiError {
    ApiError::bad_request(
        "export.nzb_unavailable",
        "The NZB behind this package is no longer kept; its history was removed",
    )
}

/// The first failures as a header value: their JSON, percent-encoded so a name in any script
/// stays inside the header's ASCII, the way `decodeURIComponent` reads it back. Empty for none.
pub(super) fn failure_header(failed: &[ExportFailure]) -> String {
    if failed.is_empty() {
        return String::new();
    }
    let first: Vec<serde_json::Value> = failed
        .iter()
        .take(HEADER_FAILURES)
        .map(|failure| {
            serde_json::json!({
                "name": failure.name,
                "message": failure.reason.message.chars().take(200).collect::<String>(),
                "code": failure.reason.code,
                "params": failure.reason.params,
            })
        })
        .collect();
    let json = serde_json::to_string(&first).unwrap_or_default();
    let mut encoded = String::with_capacity(json.len());
    for byte in json.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{ExportFailure, failure_header};

    #[test]
    fn the_header_names_a_failure_in_ascii_that_reads_back() {
        assert_eq!(failure_header(&[]), "");
        let failed: Vec<ExportFailure> = (0..12)
            .map(|_| ExportFailure {
                name: "\u{dc}n\u{ef}code Release".to_owned(),
                reason: crate::dto::MessageResponse::new("collector.nzb_rejected", "limit reached"),
            })
            .collect();
        let header = failure_header(&failed);
        assert!(header.is_ascii());
        assert!(!header.contains(' '));
        let decoded: String = percent_decoded(&header);
        let read: serde_json::Value = serde_json::from_str(&decoded).expect("JSON");
        let list = read.as_array().expect("a list");
        assert_eq!(list.len(), 10, "at most ten are named");
        assert_eq!(list[0]["name"], "\u{dc}n\u{ef}code Release");
        assert_eq!(list[0]["code"], "collector.nzb_rejected");
    }

    fn percent_decoded(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("hex");
                out.push(u8::from_str_radix(hex, 16).expect("byte"));
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(out).expect("UTF-8")
    }
}
