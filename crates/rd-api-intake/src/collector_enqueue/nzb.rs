//! NZB links: fetched, judged and imported into the Usenet queue (RD-080-11).

use rd_api_core::nzb_candidate::FetchedNzb;

use super::*;

/// Fetches an NZB link and imports it into the Usenet queue (RD-080-11).
///
/// Deliberately the same `add_nzb_import` + `enqueue_nzb_import` path a dropped file or an
/// upload takes, so segment scheduling, PAR2 handling and post-processing behave identically
/// however the NZB arrived. The alternative — letting the ordinary HTTP engine save the
/// document into the download folder — produces a `.nzb` file on disk and no download, which
/// is what happened before this existed.
///
/// This is also what makes an NZBHydra or Prowlarr link work: those return a redirect to the
/// real indexer, and following it here is the "send the link to the downloader" behaviour
/// those proxies ask for rather than having them proxy the bytes themselves.
///
/// The fetch, the address rule (`reach`, RD-150-03), the indexer's refusal and the release
/// name are `rd_api_core::nzb_candidate`'s, shared with the package export (RD-1220-02).
pub(super) async fn import_nzb_candidate(
    state: &AppState,
    candidate: &LinkCandidate,
    package: &rd_core::CollectorPackage,
    destination: &std::path::Path,
    start_paused: bool,
    reach: Option<bool>,
) -> Result<rd_core::DownloadPackage, ApiError> {
    let fetched = rd_api_core::nzb_candidate::fetch_nzb_candidate(state, candidate, reach).await?;
    let release = fetched.release(state, candidate, package).await?;
    let FetchedNzb { fetched, document } = fetched;
    let files: Vec<rd_db::NewNzbFile> = document
        .files
        .into_iter()
        .map(|file| rd_db::NewNzbFile {
            subject: file.subject,
            poster: file.poster,
            groups: file.groups,
            segments: file
                .segments
                .into_iter()
                .map(|segment| rd_db::NewNzbSegment {
                    number: segment.number,
                    bytes: segment.bytes,
                    message_id: segment.message_id,
                })
                .collect(),
        })
        .collect();
    let import = state
        .database
        .add_nzb_import(rd_db::NewNzbImport {
            name: release.name,
            sha256: rd_api_core::input_checks::sha256_hex(&fetched.bytes),
            category_id: package.category_id,
            // The package's category was already decided when the link entered the
            // LinkGrabber; this only labels the intake for the rare package without one.
            source: rd_core::IngressSource::Nzb,
            priority: Some(package.priority),
            import_mode: rd_core::ImportMode::Enqueue,
            // The address can carry an indexer API key; only the redacted form is stored.
            source_path: Some(rd_core::redact_url(&candidate.url)),
            password: release.password,
            // The links were announced when they entered the LinkGrabber; the files inside
            // this NZB are not a second arrival.
            announce_arrival: false,
            files,
        })
        .await?;
    Ok(state
        .database
        .enqueue_nzb_import(
            import.id,
            destination.to_path_buf(),
            package.priority,
            start_paused,
        )
        .await?)
}
