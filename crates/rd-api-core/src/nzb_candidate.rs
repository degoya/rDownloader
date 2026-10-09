//! An NZB link of the LinkGrabber, fetched and judged (RD-080-11).
//!
//! Two areas need the document behind such a link: the enqueue, which imports it into the
//! Usenet queue, and the package export, which carries it in an `.rdlinks` file instead of the
//! indexer's address and key (RD-1220-02). Both fetch it here, so the address rule, the
//! indexer's refusal and the release name are one implementation, not two that drift apart.
//!
//! An NZB link a document or a page proposed (`reach`, RD-150-03) is fetched on the same terms
//! as its online check: never from this machine, from the person's own network only when they
//! handed the document over, judged before the request and held to the rule by the client's
//! resolver when it connects.

use rd_core::LinkCandidate;

use crate::{ApiError, AppState};

/// The document behind an NZB link, parsed, with the answer it came in.
pub struct FetchedNzb {
    pub fetched: rd_http::FetchedDocument,
    pub document: rd_collector::NzbDocument,
}

/// What the queue calls an NZB and the archive password it takes.
pub struct NzbRelease {
    /// Sanitised, without `.nzb` and without a `{{password}}` marker.
    pub name: String,
    pub password: Option<String>,
}

/// Fetches the NZB behind `candidate`, refuses an indexer's refusal and parses the answer.
///
/// # Errors
///
/// `collector.nzb_internal_address` (`422`) for a proposed address into this machine or a
/// network it may not reach, `collector.nzb_client_unavailable`, `collector.nzb_fetch_failed`
/// and `collector.nzb_rejected` (`502`), `collector.nzb_invalid` and `collector.nzb_empty`
/// (`422`). No message carries the address with its key.
pub async fn fetch_nzb_candidate(
    state: &AppState,
    candidate: &LinkCandidate,
    reach: Option<bool>,
) -> Result<FetchedNzb, ApiError> {
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
    if document.files.is_empty() {
        return Err(ApiError::unprocessable(
            "collector.nzb_empty",
            "That NZB contains no files",
        ));
    }
    Ok(FetchedNzb { fetched, document })
}

impl FetchedNzb {
    /// The release name and archive password the queue takes for this NZB.
    ///
    /// The name from [`declared_nzb_name`], or the package's. The password: a `{{password}}`
    /// marker in the name first — it was written onto this very file, while the package's
    /// password may have been meant for a sibling link — then the package's, then the NZB's
    /// own `<meta type="password">`. That branch used to read the marker and nothing else, so a
    /// password the package already held was silently dropped for exactly the Usenet hits that
    /// most often need one.
    ///
    /// # Errors
    ///
    /// When the package's password cannot be read from the vault.
    pub async fn release(
        &self,
        state: &AppState,
        candidate: &LinkCandidate,
        package: &rd_core::CollectorPackage,
    ) -> Result<NzbRelease, ApiError> {
        let name =
            declared_nzb_name(candidate, &self.fetched).unwrap_or_else(|| package.name.clone());
        let (name, marker_password) =
            rd_files::strip_password_marker(name.trim_end_matches(".nzb"));
        let package_password = state
            .database
            .collector_package_password(package.id)
            .await?;
        Ok(NzbRelease {
            name: rd_files::sanitize_file_name(&name),
            password: marker_password
                .or(package_password)
                .or_else(|| self.document.password.clone()),
        })
    }
}

/// The release name for an NZB, in the order the sources deserve to be trusted.
///
/// The feed item's title first: it is what the subscription reviewed and what the user saw
/// in the LinkGrabber, so the queue must not call the job something else. Then the two
/// headers an indexer answers with — `X-DNZB-Name` is the release, `Content-Disposition` the
/// file it would have saved as — both of which SABnzbd reads for the same reason. The
/// address's own last segment comes last: it is `api` for every hit of an indexer.
#[must_use]
pub fn declared_nzb_name(
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
#[must_use]
pub fn indexer_refusal(fetched: &rd_http::FetchedDocument) -> Option<String> {
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
