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
///
/// The clipboard is not among them (RD-1190-18): the capture agent hands over what was copied
/// without a click, and any web page can put an address there.
pub(crate) fn from_own_hand(source: IngressSource) -> bool {
    matches!(source, IngressSource::Manual | IngressSource::HotFolder)
}

/// The source a batch that came through the capture door is recorded with (RD-1190-18).
///
/// The door is the capture agent's and the extension's, never the person's own hand, so the
/// source is pinned by the way in rather than taken from the body: a capture token naming
/// `manual` — or a hot folder, a feed or an NZB, which only the service itself names — is
/// recorded as `api`. The four a capture client really is keep their name, which routing
/// rules match on.
pub(crate) fn captured(claimed: IngressSource) -> IngressSource {
    match claimed {
        IngressSource::Clipboard
        | IngressSource::ClickAndLoad
        | IngressSource::BrowserExtension
        | IngressSource::BrowserDownload => claimed,
        IngressSource::Manual
        | IngressSource::Api
        | IngressSource::Nzb
        | IngressSource::HotFolder
        | IngressSource::Subscription => IngressSource::Api,
    }
}

/// The source an intake on the LinkGrabber route is recorded with (RD-1190-22). A token is a
/// program, never the person's own hand, so an API token naming `manual` is recorded the way
/// the capture door records a capture token ([`captured`]); a session, and a caller on this
/// machine while the login is switched off, keep the source they name.
pub(crate) fn of_caller(
    audit: &crate::audit::AuditContext,
    claimed: IngressSource,
) -> IngressSource {
    if audit.actor.kind == rd_core::AuditActorKind::Token {
        captured(claimed)
    } else {
        claimed
    }
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

    use super::{captured, from_own_hand, of_caller};
    use crate::audit::{Actor, AuditContext};

    #[test]
    fn only_a_document_from_the_persons_own_hand_may_reach_their_network() {
        for own in [IngressSource::Manual, IngressSource::HotFolder] {
            assert!(from_own_hand(own), "{own:?}");
        }
        for relayed in [
            IngressSource::Clipboard,
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

    /// RD-1190-18: whatever a capture token claims, nothing that came through the capture door
    /// counts as the person's own hand.
    #[test]
    fn the_capture_door_never_counts_as_the_persons_own_hand() {
        for claimed in [
            IngressSource::Manual,
            IngressSource::Clipboard,
            IngressSource::ClickAndLoad,
            IngressSource::Api,
            IngressSource::Nzb,
            IngressSource::HotFolder,
            IngressSource::BrowserExtension,
            IngressSource::BrowserDownload,
            IngressSource::Subscription,
        ] {
            assert!(!from_own_hand(captured(claimed)), "{claimed:?}");
        }
        assert_eq!(captured(IngressSource::Manual), IngressSource::Api);
        assert_eq!(
            captured(IngressSource::ClickAndLoad),
            IngressSource::ClickAndLoad
        );
    }

    /// RD-1190-22: on the LinkGrabber route a token is held like the capture door holds one;
    /// a session keeps the source it names.
    #[test]
    fn a_token_on_the_linkgrabber_route_never_counts_as_the_persons_own_hand() {
        let token = AuditContext {
            actor: Actor {
                kind: rd_core::AuditActorKind::Token,
                id: Some("token".to_owned()),
                label: None,
                via: rd_core::AuditChannel::Rest,
            },
            trace: None,
        };
        assert_eq!(of_caller(&token, IngressSource::Manual), IngressSource::Api);
        assert_eq!(
            of_caller(&token, IngressSource::Clipboard),
            IngressSource::Clipboard
        );
        let session = AuditContext {
            actor: Actor::session("session"),
            trace: None,
        };
        assert_eq!(
            of_caller(&session, IngressSource::Manual),
            IngressSource::Manual
        );
    }
}
