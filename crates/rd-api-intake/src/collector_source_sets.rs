//! Source sets from intake parsers, kept on the candidates they belong to (RD-150-03).
//!
//! A Metalink parser proposes one link per file and states the file's other sources beside
//! it. The link goes through the ordinary review; the set waits on the candidate's row and
//! becomes the download's sources when the candidate is queued.

use rd_core::{IngressSource, LinkCandidate, SourceSet};

use crate::{ApiError, AppState};

/// Whether a document came from the person's own hand — typed or pasted into the interface,
/// or dropped into a watched folder — rather than relayed from a web page, a feed or another
/// program. Only then may its mirrors point into the person's own network (RD-150-03): a
/// Metalink a website serves is written by that website, and its mirrors must not be able to
/// make the service reach the router or another machine behind the firewall.
pub(crate) fn from_own_hand(source: IngressSource) -> bool {
    matches!(
        source,
        IngressSource::Manual | IngressSource::Clipboard | IngressSource::HotFolder
    )
}

/// Keeps each checked source set on the candidate proposed under its address.
///
/// The domain blocklist applies to the mirrors exactly as it does to the link: a source on an
/// excluded host is taken out of the set. A set whose address did not survive the intake —
/// excluded, or its service switched off — has no candidate to go to and is dropped.
///
/// `local_network` is [`from_own_hand`] of the intake; the transfer holds every source to it.
pub(crate) async fn attach(
    state: &AppState,
    candidates: &[LinkCandidate],
    sets: Vec<(url::Url, SourceSet)>,
    excluded: &[String],
    local_network: bool,
) -> Result<(), ApiError> {
    for (primary, mut set) in sets {
        set.local_network = local_network;
        let Some(candidate) = candidates.iter().find(|candidate| candidate.url == primary) else {
            continue;
        };
        set.sources.retain(|source| {
            !source
                .url
                .host_str()
                .is_some_and(|host| crate::collector_exclusions::is_excluded(excluded, host))
        });
        if set.sources.is_empty() {
            continue;
        }
        state
            .database
            .set_candidate_source_set(candidate.id, set)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rd_core::IngressSource;

    use super::from_own_hand;

    #[test]
    fn only_a_document_from_the_persons_own_hand_may_reach_their_network() {
        for own in [
            IngressSource::Manual,
            IngressSource::Clipboard,
            IngressSource::HotFolder,
        ] {
            assert!(from_own_hand(own), "{own:?}");
        }
        for relayed in [
            IngressSource::BrowserExtension,
            IngressSource::BrowserDownload,
            IngressSource::ClickAndLoad,
            IngressSource::Api,
            IngressSource::Nzb,
            IngressSource::Subscription,
        ] {
            assert!(!from_own_hand(relayed), "{relayed:?}");
        }
    }
}
