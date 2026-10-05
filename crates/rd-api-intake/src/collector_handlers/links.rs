//! One link on its way into a LinkGrabber batch, with what the intake keeps alongside it.

use crate::AppState;

/// URLs, file names, sizes, package hints, mirror hints, captured requests and vaulted body
/// references, parallel per link.
pub(super) type SplitLinks = (
    Vec<url::Url>,
    Vec<Option<String>>,
    Vec<Option<rd_core::ByteCount>>,
    Vec<Option<String>>,
    Vec<Option<rd_core::MirrorHint>>,
    Vec<Option<rd_core::CapturedRequest>>,
    Vec<Option<String>>,
);

/// One link on its way into a batch, with the metadata the intake keeps parallel to it.
pub(super) struct CapturedLink {
    pub(super) url: url::Url,
    pub(super) file_name: Option<String>,
    /// Size the source stated, when it did. A crawler reads it out of the folder listing;
    /// nothing else here knows one before the link check has run.
    pub(super) size: Option<rd_core::ByteCount>,
    /// The package this link's source says it belongs to — the folder a crawler found it in.
    pub(super) package_hint: Option<String>,
    /// What the source said about this link being one of several copies of the same file
    /// (RD-110-18). Only a crawler or a site rule ever knows this; a pasted link does not.
    pub(super) mirror: Option<rd_core::MirrorHint>,
    pub(super) request: Option<rd_core::CapturedRequest>,
    /// `vault://` reference of the encrypted request body, when one was stored.
    pub(super) body_ref: Option<String>,
    /// Who named the address — decides whether its online check is held to an address rule.
    pub(super) origin: LinkOrigin,
}

/// Who put a link into an intake (RD-150-03).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkOrigin {
    /// Typed, pasted or sent by the person: checked as it always was.
    Person,
    /// Proposed by an intake parser out of a document — a Metalink's link among them.
    Proposed,
    /// Found by a crawler on a page.
    Crawled,
}

impl LinkOrigin {
    /// Whether the online check of such a link is held to an address rule. A parser's
    /// proposal always is: the document was written by somebody else even when the person
    /// pasted it. A crawler's find only when the page did not come from the person's own hand:
    /// a folder they pointed the crawler at themselves is their own network use.
    pub(super) fn guarded(self, own_hand: bool) -> bool {
        match self {
            Self::Person => false,
            Self::Proposed => true,
            Self::Crawled => !own_hand,
        }
    }
}

impl CapturedLink {
    /// A link extracted from free text: no client-supplied metadata.
    pub(super) fn plain(url: url::Url) -> Self {
        Self {
            url,
            file_name: None,
            size: None,
            package_hint: None,
            mirror: None,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Person,
        }
    }

    /// A link an intake parser proposed. It carries a name and nothing else: a plugin never
    /// supplies request metadata, because that would be a credential path it has no claim to.
    pub(super) fn proposed(url: url::Url, file_name: Option<String>) -> Self {
        Self {
            url,
            file_name: file_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            size: None,
            package_hint: None,
            mirror: None,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Proposed,
        }
    }

    /// A link a folder crawler found (RD-104-03).
    ///
    /// It carries what the folder listing stated — a name, a size and the folder it sat in —
    /// and nothing else. Everything below this point treats it exactly as a pasted link:
    /// the blocklist, the disabled-service refusal, the review and the routing rules all
    /// apply, so a crawler queues nothing by itself.
    pub(super) fn crawled(link: rd_plugin_ext::CrawledLink) -> Self {
        Self {
            url: link.url,
            file_name: link.file_name,
            size: link
                .size
                .and_then(|size| rd_core::ByteCount::new(size).ok()),
            package_hint: link.package_hint,
            mirror: link.mirror,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Crawled,
        }
    }

    /// Splits into the parallel vectors `NewCollectorBatch` expects.
    pub(super) fn split(links: Vec<Self>) -> SplitLinks {
        let mut urls = Vec::with_capacity(links.len());
        let mut file_names = Vec::with_capacity(links.len());
        let mut sizes = Vec::with_capacity(links.len());
        let mut package_hints = Vec::with_capacity(links.len());
        let mut mirror_hints = Vec::with_capacity(links.len());
        let mut requests = Vec::with_capacity(links.len());
        let mut body_refs = Vec::with_capacity(links.len());
        for link in links {
            urls.push(link.url);
            file_names.push(link.file_name);
            sizes.push(link.size);
            package_hints.push(link.package_hint);
            mirror_hints.push(link.mirror);
            requests.push(link.request);
            body_refs.push(link.body_ref);
        }
        (
            urls,
            file_names,
            sizes,
            package_hints,
            mirror_hints,
            requests,
            body_refs,
        )
    }

    /// Removes every body this intake vaulted.
    ///
    /// Called when the batch is abandoned after the bodies were already encrypted, so a
    /// rejected or fully-excluded capture cannot leave orphaned ciphertext behind.
    pub(super) async fn discard_bodies(state: &AppState, links: &[Self]) {
        for link in links {
            if let Some(reference) = &link.body_ref
                && let Err(error) = state.secrets.remove(reference).await
            {
                tracing::warn!(%error, "could not remove an unused captured request body");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LinkOrigin;

    /// RD-150-03: what a document or a stranger's page proposed is checked under the address
    /// rule; what the person gave, and a folder they pointed the crawler at, is not.
    #[test]
    fn only_links_somebody_else_proposed_are_checked_under_the_address_rule() {
        for own_hand in [true, false] {
            assert!(!LinkOrigin::Person.guarded(own_hand));
            assert!(LinkOrigin::Proposed.guarded(own_hand));
        }
        assert!(!LinkOrigin::Crawled.guarded(true));
        assert!(LinkOrigin::Crawled.guarded(false));
    }
}
