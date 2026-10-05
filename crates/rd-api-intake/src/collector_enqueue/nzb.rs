//! NZB links: fetched, judged and imported into the Usenet queue (RD-080-11).

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
/// An NZB link a document or a page proposed (`reach`, RD-150-03) is fetched on the same
/// terms as its online check: never from this machine, from the person's own network only
/// when they handed the document over, judged before the request and held to the rule by
/// the client's resolver when it connects.
pub(super) async fn import_nzb_candidate(
    state: &AppState,
    candidate: &LinkCandidate,
    package: &rd_core::CollectorPackage,
    destination: &std::path::Path,
    start_paused: bool,
    reach: Option<bool>,
) -> Result<rd_core::DownloadPackage, ApiError> {
    let network = match reach {
        Some(local_network) => {
            let policy = state.scheduler.remote_address_policy(local_network);
            if let Err(rd_http::TargetRefusal::Refused(_)) =
                rd_http::check_target(&policy, &rd_http::SystemLookup, &candidate.url).await
            {
                return Err(ApiError::unprocessable(
                    crate::error_codes::NZB_INTERNAL_ADDRESS,
                    "This NZB link points at this machine or into your own network, so it was not fetched",
                )
                .with_param("candidate_id", candidate.id));
            }
            state.scheduler.guarded_client(&candidate.url, policy).await
        }
        None => state.scheduler.direct_client(&candidate.url).await,
    }
    .map_err(|error| {
        ApiError::bad_gateway("collector.nzb_client_unavailable", error.to_string())
            .with_param("reason", error)
    })?;
    let fetched = rd_http::fetch_document(
        &network.client,
        candidate.url.clone(),
        &network.headers,
        rd_collector::MAX_NZB_BYTES,
    )
    .await
    .map_err(|error| {
        // The address can carry an indexer API key, so it never reaches the message.
        let reason = format!("{error} ({})", rd_core::redact_url(&candidate.url));
        ApiError::bad_gateway("collector.nzb_fetch_failed", reason.clone())
            .with_param("reason", reason)
    })?;
    // An indexer refuses inside a `200 OK`: the API limit is reached, the key is wrong, the
    // release is gone. Without this the body fails to parse and the user is told the NZB is
    // invalid, which sends them looking in the wrong place.
    if let Some(refusal) = indexer_refusal(&fetched) {
        return Err(
            ApiError::bad_gateway("collector.nzb_rejected", refusal.clone())
                .with_param("reason", refusal),
        );
    }
    let document = rd_collector::parse_nzb(&fetched.bytes).map_err(|error| {
        ApiError::unprocessable("collector.nzb_invalid", error.to_string())
            .with_param("reason", error)
    })?;
    let name = declared_nzb_name(candidate, &fetched).unwrap_or_else(|| package.name.clone());
    let (name, marker_password) = rd_files::strip_password_marker(name.trim_end_matches(".nzb"));
    // This branch used to read the file-name marker and nothing else, so a password the
    // package already held -- announced by a subscription, a DLC container or the API --
    // was silently dropped for exactly the Usenet hits that most often need one. The marker
    // still wins: it was written onto this very file, while the package's password may have
    // been meant for a sibling link.
    let package_password = state
        .database
        .collector_package_password(package.id)
        .await?;
    let password = marker_password
        .or(package_password)
        .or_else(|| document.password.clone());
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
    if files.is_empty() {
        return Err(ApiError::unprocessable(
            "collector.nzb_empty",
            "That NZB contains no files",
        ));
    }
    let import = state
        .database
        .add_nzb_import(rd_db::NewNzbImport {
            name: rd_files::sanitize_file_name(&name),
            sha256: rd_api_core::input_checks::sha256_hex(&fetched.bytes),
            category_id: package.category_id,
            // The package's category was already decided when the link entered the
            // LinkGrabber; this only labels the intake for the rare package without one.
            source: rd_core::IngressSource::Nzb,
            priority: Some(package.priority),
            import_mode: rd_core::ImportMode::Enqueue,
            // The address can carry an indexer API key; only the redacted form is stored.
            source_path: Some(rd_core::redact_url(&candidate.url)),
            password,
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

/// The release name for an NZB, in the order the sources deserve to be trusted.
///
/// The feed item's title first: it is what the subscription reviewed and what the user saw
/// in the LinkGrabber, so the queue must not call the job something else. Then the two
/// headers an indexer answers with — `X-DNZB-Name` is the release, `Content-Disposition` the
/// file it would have saved as — both of which SABnzbd reads for the same reason. The
/// address's own last segment comes last: it is `api` for every hit of an indexer.
pub(super) fn declared_nzb_name(
    candidate: &LinkCandidate,
    fetched: &rd_http::FetchedDocument,
) -> Option<String> {
    let clean = |value: &str| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    };
    candidate
        .file_name_declared
        .then_some(candidate.file_name.as_deref())
        .flatten()
        .and_then(clean)
        .or_else(|| fetched.header("x-dnzb-name").and_then(clean))
        .or_else(|| {
            fetched
                .header("content-disposition")
                .and_then(crate::link_check_probe::disposition_file_name)
                .as_deref()
                .and_then(clean)
        })
        .or_else(|| candidate.file_name.as_deref().and_then(clean))
}

/// The indexer's own refusal, when it answered `200 OK` with something that is not an NZB.
///
/// The `X-DNZB-*` headers SABnzbd established: `X-DNZB-Failure` states the reason outright,
/// and an `X-DNZB-RCode` other than 200 carries it in `X-DNZB-RText` ("Request limit
/// reached"). Both are worth more than the parse error the body would produce.
pub(crate) fn indexer_refusal(fetched: &rd_http::FetchedDocument) -> Option<String> {
    if let Some(failure) = fetched
        .header("x-dnzb-failure")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(failure.to_owned());
    }
    let code = fetched.header("x-dnzb-rcode")?.trim();
    if code == "200" {
        return None;
    }
    let text = fetched
        .header("x-dnzb-rtext")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("the indexer refused the download");
    Some(format!("{text} (code {code})"))
}
