//! The `.torrent` behind a LinkGrabber link, read on the link's own network (RD-1320-05).
//!
//! *Resolve* on a torrent link handed librqbit the address, and librqbit fetched it with a
//! client of its own: no authentication profile, no proxy, no custom CA, no address rule. An
//! http(s) address is fetched here instead, the way the link's online check reads it — the
//! link's own profile choice, the global proxy and CA, and for a link a document or a page
//! proposed (`reach`, RD-150-03) the address rule before the request and in the resolver — and
//! its metadata read from the bytes. The bytes are kept for the link's download (RD-130-18), so
//! queueing it does not ask the tracker a second time. A magnet still goes to the engine: its
//! metadata only comes from the swarm.

use anyhow::Result;
use rd_core::{LinkCandidate, TorrentMetadataInfo};

use crate::AppState;

/// The metadata behind `candidate`: read from the `.torrent` at an http(s) address, or asked of
/// the swarm for a magnet.
///
/// # Errors
///
/// The address is refused by the rule, cannot be fetched or is no torrent; the engine cannot
/// resolve the magnet. No message carries the address, which may hold a tracker's passkey.
pub async fn resolve_torrent_candidate(
    state: &AppState,
    candidate: &LinkCandidate,
) -> Result<TorrentMetadataInfo> {
    if !matches!(candidate.url.scheme(), "http" | "https") {
        return state.torrent.resolve_metadata(&candidate.url).await;
    }
    let bytes = fetch(state, candidate).await?;
    let parsed = rd_torrent::parse_torrent(&bytes)?;
    if let Err(error) = state.torrent.keep_prefetched(candidate.id, &bytes).await {
        // Costs the download a second fetch, nothing more.
        tracing::warn!(%error, "a resolved torrent could not be kept for its download");
    }
    Ok(parsed.metadata)
}

/// Fetches the `.torrent` at the candidate's address on its own network.
async fn fetch(state: &AppState, candidate: &LinkCandidate) -> Result<Vec<u8>> {
    let reach = state.database.candidate_remote_reach(candidate.id).await?;
    let network = match reach {
        Some(local_network) => {
            let policy = state.scheduler.remote_address_policy(local_network);
            if let Err(rd_http::TargetRefusal::Refused(_)) =
                rd_http::check_target(&policy, &rd_http::SystemLookup, &candidate.url).await
            {
                anyhow::bail!(
                    "the address points at this machine or into your own network, so it was not fetched"
                );
            }
            state
                .scheduler
                .guarded_client(&candidate.url, candidate.auth_profile, policy)
                .await?
        }
        None => {
            state
                .scheduler
                .direct_client(&candidate.url, candidate.auth_profile)
                .await?
        }
    };
    // The bound the online check reads a torrent within.
    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rd_http::fetch_document(
            &network.client,
            candidate.url.clone(),
            &network.headers,
            rd_torrent::MAX_TORRENT_BYTES,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the .torrent file did not arrive in time"))?
    .map_err(|error| match error.downcast::<reqwest::Error>() {
        Ok(error) => anyhow::Error::new(error.without_url()),
        Err(error) => error,
    })?;
    Ok(fetched.bytes)
}
