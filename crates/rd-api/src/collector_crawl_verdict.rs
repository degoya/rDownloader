//! The verdict a crawled address passes before it may become a candidate (RD-110-07).
//!
//! A crawler -- plugin or site rule -- may answer with any address it likes; the WIT contract
//! says so in as many words, and it has to, because the file behind a release page usually
//! sits on a different host. Nothing checked what came back. A rule whose container pattern
//! reaches one element too far therefore produced a candidate for the board page itself, and
//! the queue later stored that page under the name of an episode.
//!
//! So every crawled address is sorted into one of four cases before a row exists for it:
//!
//! 1. something already speaks for it -- a resolver, a transfer backend, a provider the
//!    intake recognises. It goes through unchanged and is **not** probed: a resolver turns
//!    the address into a file later, and a HEAD against a hoster's landing page would answer
//!    `text/html` and prove nothing;
//! 2. nobody speaks for it, but a probe says the response is file content. It goes through as
//!    a direct link;
//! 3. nobody speaks for it, and the probe **answered** with a page. No candidate -- a counted
//!    refusal with a stable code instead;
//! 4. nobody speaks for it, and the probe got no answer at all -- a timeout, a refused
//!    connection, a `4xx`. The address is kept, unconfirmed.
//!
//! Case 4 is a decision, not an oversight, and it overrules this job's literal wording
//! ("neither claimed nor confirmed -- no candidate"). RD-101-06 settled the same question for
//! the online check: a link whose check *failed* stays queueable, and must not be treated
//! more harshly than one the check proved dead. Dropping on silence would make a momentary
//! network fault cost a crawler's valid links without saying so. Nothing is lost by keeping
//! it: the address goes through the ordinary online check like any other candidate, and
//! [`rd_http::ProbeResult::looks_downloadable`] is applied a second time mid-transfer, so a
//! page that slipped through here still never lands on disk as a file.
//!
//! The judgement in cases 2 and 3 is [`rd_http::ProbeResult::looks_downloadable`], asked
//! rather than rebuilt. That function is also what the transfer engine applies mid-download
//! and what the online check applies to a direct link, and a second copy of it here would be
//! a second opinion to drift.

use std::future::Future;

use url::Url;

use crate::AppState;

/// How long one probe may take before the address counts as unreachable.
///
/// The same budget the online check gives its own direct probe. An intake that crawled a
/// folder of five hundred links asks this question for each of them that nothing claims, so
/// a host that accepts a connection and then says nothing must not hold the whole paste.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long the whole pass may spend probing, across all of an intake's addresses.
///
/// The per-address timeout alone bounds nothing that matters: the pass is sequential and runs
/// inside the intake request, so a folder of five hundred unclaimed links behind a host that
/// accepts connections and then says nothing would hold that request open for hours, and the
/// caller would give up long before the handler answered -- losing the links that *would* have
/// worked along with the slow ones. Once this budget is spent, the remaining addresses are not
/// probed at all and go through as [`CrawlVerdict::Unconfirmed`], which is the safe direction:
/// unproven means kept, and `looks_downloadable` still judges the response mid-transfer.
pub(crate) const PROBE_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// What asking about one crawled address produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CrawlVerdict {
    /// A resolver, a transfer backend or a recognised provider speaks for this address.
    Claimed,
    /// Nobody speaks for it, and a probe read file content behind it.
    Confirmed,
    /// Nobody speaks for it, and the response is a page rather than a file.
    NotAFile,
    /// Nobody speaks for it, and the probe got no answer at all. Kept anyway -- see the
    /// module documentation, and RD-101-06.
    Unconfirmed,
}

impl CrawlVerdict {
    /// Whether a candidate may be created for this address.
    ///
    /// Only a probe that *answered* with a page refuses one. Silence does not.
    pub(crate) fn keeps_the_link(self) -> bool {
        !matches!(self, Self::NotAFile)
    }

    /// The stable word the trial run of a rule prints beside an address (RD-110-08). Not a
    /// translated one: the interface has four sentences for these four cases.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Confirmed => "confirmed",
            Self::NotAFile => "not-a-file",
            Self::Unconfirmed => "unconfirmed",
        }
    }

    /// The stable code a dropped address is reported under, `None` when it was kept.
    ///
    /// One code, because there is one refusal. An address that never answered is kept, so it
    /// has nothing to report here; what it has to say, it says through its own online check.
    pub(crate) fn code(self) -> Option<&'static str> {
        match self {
            Self::Claimed | Self::Confirmed | Self::Unconfirmed => None,
            Self::NotAFile => Some("collector.crawl_not_a_file"),
        }
    }

    /// The English fallback beside the code, for a reader whose catalogue has neither.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Claimed | Self::Confirmed | Self::Unconfirmed => "",
            Self::NotAFile => {
                "The crawler returned addresses that answer with a page rather than a file"
            }
        }
    }
}

/// Who already speaks for one address, each reason kept apart so a log line can name it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Claim {
    /// The intake recognises a provider for it: media, gallery, torrent, NZB, FTP/SFTP/WebDAV.
    pub(crate) provider: bool,
    /// An installed transfer backend claims the address's scheme.
    pub(crate) transfer_scheme: bool,
    /// An installed resolver matches it, free or account-bound.
    pub(crate) resolver: bool,
    /// A provider plugin registers the host, so this is a hoster link whether or not the
    /// resolver that serves it is loaded right now.
    pub(crate) known_hoster: bool,
}

impl Claim {
    /// Whether anything at all speaks for the address.
    pub(crate) fn is_claimed(self) -> bool {
        self.provider || self.transfer_scheme || self.resolver || self.known_hoster
    }
}

/// What this installation knows about one crawled address, without touching the network.
pub(crate) fn claim(
    state: &AppState,
    url: &Url,
    media: &rd_core::MediaSettings,
    gallery: &rd_core::GallerySettings,
) -> Claim {
    // The same routing the intake itself performs a few lines later, asked here so the two
    // cannot disagree about what a provider link is.
    let provider =
        crate::collector_handlers::providers_for(std::slice::from_ref(url), media, gallery)
            .first()
            .is_some_and(Option::is_some);
    Claim {
        provider,
        transfer_scheme: state
            .plugin_transfer_schemes
            .iter()
            .any(|scheme| scheme.eq_ignore_ascii_case(url.scheme())),
        resolver: state.scheduler.resolvers().has_resolver(url),
        known_hoster: rd_provider_registry::provider_for_url(url).is_some(),
    }
}

/// The verdict on one crawled address, probing only while the pass's budget lasts.
pub(crate) async fn verdict(
    state: &AppState,
    url: &Url,
    media: &rd_core::MediaSettings,
    gallery: &rd_core::GallerySettings,
    deadline: std::time::Instant,
) -> CrawlVerdict {
    decide(
        claim(state, url, media, gallery),
        std::time::Instant::now() < deadline,
        || probe(state, url),
    )
    .await
}

/// The decision itself, with the probe left to the caller.
///
/// Split out so the two rules that matter most can be tested for what they do **not** do:
/// neither a claimed address nor one past the pass's budget may reach the network. A test that
/// only compared verdicts would pass while the probe ran anyway.
async fn decide<F, Fut>(claim: Claim, may_probe: bool, probe: F) -> CrawlVerdict
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = CrawlVerdict>,
{
    if claim.is_claimed() {
        return CrawlVerdict::Claimed;
    }
    if !may_probe {
        return CrawlVerdict::Unconfirmed;
    }
    probe().await
}

async fn probe(state: &AppState, url: &Url) -> CrawlVerdict {
    let Ok(network) = state.scheduler.direct_client(url).await else {
        return CrawlVerdict::Unconfirmed;
    };
    confirm(&network.client, &network.headers, url.clone()).await
}

/// Reads the headers behind one address and applies the one file judgement to them.
pub(crate) async fn confirm(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: Url,
) -> CrawlVerdict {
    match tokio::time::timeout(
        PROBE_TIMEOUT,
        rd_http::probe_with_headers(client, url, headers),
    )
    .await
    {
        Ok(Ok(result)) if result.looks_downloadable() => CrawlVerdict::Confirmed,
        Ok(Ok(_)) => CrawlVerdict::NotAFile,
        // A refused connection, a 4xx, a timeout. Nothing was proven either way, and an
        // unproven address is not a refused one (RD-101-06): it is kept and checked like
        // every other candidate.
        _ => CrawlVerdict::Unconfirmed,
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};

    use super::{Claim, CrawlVerdict, confirm, decide};

    fn nothing() -> Claim {
        Claim::default()
    }

    /// A hoster link a resolver claims is never probed. The resolver produces the file, and
    /// the page at that address is a landing page that would fail the test on its own.
    #[tokio::test]
    async fn a_claimed_address_is_not_probed() {
        for claim in [
            Claim {
                resolver: true,
                ..nothing()
            },
            Claim {
                known_hoster: true,
                ..nothing()
            },
            Claim {
                provider: true,
                ..nothing()
            },
            Claim {
                transfer_scheme: true,
                ..nothing()
            },
        ] {
            let mut probed = false;
            let verdict = decide(claim, true, || {
                probed = true;
                async { CrawlVerdict::NotAFile }
            })
            .await;
            assert_eq!(verdict, CrawlVerdict::Claimed);
            assert!(!probed, "a claimed address must not reach the network");
        }
    }

    /// The bound the pass needs, because it is sequential and runs inside the intake request:
    /// once the budget is spent nothing else is probed. What is left goes through unproven --
    /// kept, which is the safe direction, and still judged again mid-transfer.
    #[tokio::test]
    async fn nothing_is_probed_once_the_budget_is_spent() {
        let mut probed = false;
        let verdict = decide(nothing(), false, || {
            probed = true;
            async { CrawlVerdict::Confirmed }
        })
        .await;
        assert_eq!(verdict, CrawlVerdict::Unconfirmed);
        assert!(verdict.keeps_the_link());
        assert!(
            !probed,
            "an address past the budget must not reach the network"
        );
    }

    #[tokio::test]
    async fn an_unclaimed_address_is_probed() {
        let mut probed = false;
        let verdict = decide(nothing(), true, || {
            probed = true;
            async { CrawlVerdict::Confirmed }
        })
        .await;
        assert_eq!(verdict, CrawlVerdict::Confirmed);
        assert!(probed);
    }

    /// The defect this job exists for, end to end at the verdict: a rule hands back the board
    /// page it read instead of the file, and the answer must be a refusal.
    #[tokio::test]
    async fn a_page_is_refused_and_an_attachment_is_kept() {
        let app = Router::new()
            .route(
                "/board/thread-4711",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        "<html><body>Season 3 Episode 4</body></html>",
                    )
                }),
            )
            .route(
                "/dl/episode",
                get(|| async {
                    (
                        [
                            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                            (
                                axum::http::header::CONTENT_DISPOSITION,
                                "attachment; filename=\"episode.mkv\"",
                            ),
                        ],
                        "payload",
                    )
                }),
            )
            .route(
                "/dl/plain",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "video/x-matroska")],
                        "payload",
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let client = reqwest::Client::new();
        let at =
            |path: &str| -> url::Url { format!("http://{address}{path}").parse().expect("URL") };
        let page = confirm(&client, &[], at("/board/thread-4711")).await;
        assert_eq!(page, CrawlVerdict::NotAFile);
        assert!(!page.keeps_the_link());
        assert_eq!(page.code(), Some("collector.crawl_not_a_file"));
        for path in ["/dl/episode", "/dl/plain"] {
            let file = confirm(&client, &[], at(path)).await;
            assert_eq!(file, CrawlVerdict::Confirmed, "{path}");
            assert!(file.keeps_the_link(), "{path}");
            assert_eq!(file.code(), None, "{path}");
        }
        let gone = confirm(&client, &[], at("/board/nothing-here")).await;
        assert_eq!(gone, CrawlVerdict::Unconfirmed);
        // RD-101-06's rule, at the crawl: silence is not a refusal. The address is kept and
        // finds out what it is through the ordinary online check.
        assert!(gone.keeps_the_link());
        assert_eq!(gone.code(), None);
    }

    /// One refusal, shaped like the rest of the collector's codes so the catalogue cannot
    /// silently miss it -- and nothing reported for an address that is kept.
    #[test]
    fn only_a_page_is_refused_and_it_carries_a_code() {
        let not_a_file = CrawlVerdict::NotAFile.code().expect("a code");
        assert!(not_a_file.starts_with("collector.crawl_"), "{not_a_file}");
        assert!(!CrawlVerdict::NotAFile.message().is_empty());
        for kept in [
            CrawlVerdict::Claimed,
            CrawlVerdict::Confirmed,
            CrawlVerdict::Unconfirmed,
        ] {
            assert!(kept.keeps_the_link(), "{kept:?}");
            assert_eq!(kept.code(), None, "{kept:?}");
            assert!(kept.message().is_empty(), "{kept:?}");
        }
    }
}
