//! One link on its way into a LinkGrabber batch, with what the intake keeps alongside it.

use rd_api_core::input_checks::optional_text;

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

/// Who put a link into an intake (RD-150-03, RD-1190-18).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkOrigin {
    /// Sent in as a link: typed or pasted by the person — or handed over by Click'n'Load, the
    /// clipboard watcher, the browser extension or a tool, where a page or a program chose it.
    Person,
    /// Proposed by an intake parser out of a document — a Metalink's link among them.
    Proposed,
    /// Found on a page: by a crawler plugin in a folder, or by a site rule (`by_rule`) on a
    /// release page.
    Crawled { by_rule: bool },
}

impl LinkOrigin {
    /// How far the online check and the transfer of such a link may reach (RD-150-03): `None`
    /// for an address the person chose themselves, which is checked as it always was, and
    /// otherwise `Some(local_network)` — never this machine, and the person's own network only
    /// when they named it.
    ///
    /// The one decision every way in shares (RD-1190-18). A link is the person's own only when
    /// the intake came from their own hand: Click'n'Load and the clipboard are filled by any
    /// web page, the extension and a tool pass on what a page or a program chose. A parser's
    /// proposal and a crawler plugin's find reach the person's network only when they handed
    /// the document or the folder over themselves — a folder they pointed the crawler at is
    /// their own network use. A site rule's find never does: the release page's operator chose
    /// that address, not the person who pasted the page.
    pub(super) fn reach(self, own_hand: bool) -> Option<bool> {
        match self {
            Self::Person => (!own_hand).then_some(false),
            Self::Proposed | Self::Crawled { by_rule: false } => Some(own_hand),
            Self::Crawled { by_rule: true } => Some(false),
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
            file_name: optional_text(file_name),
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
            origin: LinkOrigin::Crawled {
                by_rule: link.by_rule,
            },
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
    /// rule; what the person gave, and a folder they pointed the crawler at, may reach their
    /// own network. RD-1190-18: a link from any other hand — Click'n'Load, the clipboard, the
    /// extension, a tool — and a site rule's find, whoever pasted the page, reach neither this
    /// machine nor that network.
    #[test]
    fn only_the_persons_own_links_skip_the_address_rule() {
        let folder = LinkOrigin::Crawled { by_rule: false };
        let rule = LinkOrigin::Crawled { by_rule: true };
        assert_eq!(LinkOrigin::Person.reach(true), None);
        assert_eq!(LinkOrigin::Person.reach(false), Some(false));
        for own_hand in [true, false] {
            assert_eq!(LinkOrigin::Proposed.reach(own_hand), Some(own_hand));
            assert_eq!(folder.reach(own_hand), Some(own_hand));
            assert_eq!(rule.reach(own_hand), Some(false));
        }
    }
}
