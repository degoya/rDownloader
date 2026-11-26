//! An NZB link of a LinkGrabber package handed to a provider as its document (RD-1480-05).
//!
//! The link's address is an indexer's download call with the person's API key in it. The document
//! behind it is fetched as the enqueue fetches it (`rd_api_core::nzb_candidate`: the address rule
//! of a proposed link, the indexer's refusal, the release name and the archive password) and goes
//! through `rd_api_core::nzb_hand_over`, the path of an NZB import and of a subscription's hit.
//! The provider is given the document, never the address.
//!
//! A link the online check has not recognised yet is still an indexer's when it has a Newznab
//! download call's form or sits at an indexer the person set up: its key may be a part of the
//! path or a parameter no list of secret names knows (`r=`). It goes the same way, and one whose
//! answer is no NZB stays with the reason -- in neither case as an address.

use std::collections::HashSet;

use rd_api_core::{
    nzb_candidate,
    nzb_hand_over::{self, NzbHandedOver},
};
use rd_core::{AccountId, CollectorPackage, LinkCandidate, SubscriptionKind};
use url::Url;

use crate::{ApiError, AppState};

/// The shortest `r=` beside an `i=` that is taken for a Newznab key; the real ones are 32 digits.
const MIN_RSS_KEY: usize = 16;

/// Where the indexers the person set up are reached: host and, where it is not the scheme's own,
/// the port.
pub(super) struct IndexerHosts(HashSet<(String, Option<u16>)>);

impl IndexerHosts {
    /// Those of the indexer list and of the indexer subscriptions, switched-off ones included:
    /// a link of theirs still carries the key.
    pub(super) async fn read(state: &AppState) -> Result<Self, ApiError> {
        let indexers = state.database.list_indexers().await?;
        let subscriptions = state.database.list_subscriptions().await?;
        Ok(Self::of(
            indexers.iter().map(|indexer| &indexer.url).chain(
                subscriptions
                    .iter()
                    .filter(|subscription| subscription.kind == SubscriptionKind::Indexer)
                    .map(|subscription| &subscription.url),
            ),
        ))
    }

    fn of<'a>(urls: impl Iterator<Item = &'a Url>) -> Self {
        Self(urls.filter_map(Self::place).collect())
    }

    fn holds(&self, url: &Url) -> bool {
        Self::place(url).is_some_and(|place| self.0.contains(&place))
    }

    /// An indexer's API and its download calls sit under one name with and without `api.`.
    fn place(url: &Url) -> Option<(String, Option<u16>)> {
        let host = url.host_str()?.to_ascii_lowercase();
        let name = ["api.", "www."]
            .iter()
            .find_map(|prefix| host.strip_prefix(prefix))
            .unwrap_or(&host);
        Some((name.to_owned(), url.port()))
    }
}

/// Whether a link's address is an indexer's NZB call, recognised by the online check or not.
///
/// A link the check placed elsewhere -- a hoster, a torrent, a media page -- is what it found.
pub(super) fn is_nzb_call(candidate: &LinkCandidate, indexers: &IndexerHosts) -> bool {
    match candidate.provider.as_deref() {
        Some(rd_core::NZB_PROVIDER) => true,
        None | Some("direct_http") => {
            newznab_form(&candidate.url) || indexers.holds(&candidate.url)
        }
        Some(_) => false,
    }
}

/// The forms a Newznab indexer's download address takes: the API call `t=get` with an id, a
/// `getnzb` path, and the pair `i=<user>&r=<key>` -- which Newznab also writes behind the file
/// name inside the path (`/getnzb/<id>.nzb&i=1&r=<key>`), where it is no query at all.
fn newznab_form(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    if path
        .split('/')
        .any(|segment| matches!(segment, "getnzb" | "getnzb.php") || segment.starts_with("getnzb&"))
    {
        return true;
    }
    let in_path = path.split_once('&').map_or("", |(_, pairs)| pairs);
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .chain(url::form_urlencoded::parse(in_path.as_bytes()))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
        .collect();
    let has = |name: &str, fits: &dyn Fn(&str) -> bool| {
        pairs
            .iter()
            .any(|(other, value)| other == name && fits(value))
    };
    let call = has("t", &|value| {
        value.eq_ignore_ascii_case("get") || value.eq_ignore_ascii_case("g")
    }) && (has("id", &|value| !value.is_empty())
        || has("guid", &|value| !value.is_empty()));
    let keyed = has("i", &|value| {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
    }) && has("r", &|value| {
        value.len() >= MIN_RSS_KEY && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    });
    call || keyed
}

/// Fetches the NZB behind `candidate` and submits it to the account's provider as a container.
///
/// The account is judged first, so a provider that takes no NZB files costs the indexer no
/// download.
pub(super) async fn submit(
    state: &AppState,
    package: &CollectorPackage,
    candidate: &LinkCandidate,
    account_id: AccountId,
) -> Result<NzbHandedOver, ApiError> {
    nzb_hand_over::ensure_takes_nzb(state, account_id).await?;
    let reach = state.database.candidate_remote_reach(candidate.id).await?;
    let fetched = nzb_candidate::fetch_nzb_candidate(state, candidate, reach).await?;
    let release = fetched.release(state, candidate, package).await?;
    // The password the enqueue would take travels inside the document. One that already names it
    // goes as the indexer sent it, as a subscription's hit does, and a file without a post date
    // gets a fixed one: the same release is the same document, and the duplicate guard meets it.
    let content = if release.password == fetched.document.password {
        fetched.fetched.bytes
    } else {
        let mut document = fetched.document;
        document.password = release.password;
        rd_collector::render_nzb(
            &document,
            Some(&rd_collector::container_name(&release.name)),
            0,
        )
    };
    let name = rd_api_core::remote_job_service::source_name(Some(&format!("{}.nzb", release.name)));
    nzb_hand_over::submit_nzb(state, account_id, content, name).await
}

#[cfg(test)]
mod tests {
    use super::{IndexerHosts, newznab_form};

    fn url(value: &str) -> url::Url {
        value.parse().expect("url")
    }

    #[test]
    fn a_newznab_download_address_is_recognised_wherever_its_key_is() {
        for call in [
            "https://indexer.example/api?t=get&id=abc&apikey=KEY",
            "https://indexer.example/api?t=GET&guid=abc&r=KEY",
            "https://indexer.example/KEY/api?t=g&id=abc",
            "https://indexer.example/getnzb/abc.nzb&i=12&r=0123456789abcdef0123456789abcdef",
            "https://indexer.example/getnzb?id=abc&r=KEY",
            "https://indexer.example/getnzb.php?id=abc",
            "https://indexer.example/getnzb/KEY/abc",
            "https://indexer.example/sub/GetNZB/abc",
            "https://indexer.example/dl/abc?i=12&r=0123456789abcdef0123456789abcdef",
            "https://indexer.example/nzb/abc.nzb&i=12&r=0123456789abcdef0123456789abcdef",
        ] {
            assert!(newznab_form(&url(call)), "{call}");
        }
    }

    /// The links a person hands over every day, some of them close to the forms above.
    #[test]
    fn an_ordinary_link_is_not_taken_for_one() {
        for plain in [
            "https://files.example/Plain.S01E01.mkv",
            "https://files.example/getnzb-1.0.tar.gz",
            "https://files.example/tools/getnzb.exe",
            "https://forum.example/thread?t=12345&id=7",
            "https://forum.example/viewtopic.php?t=get",
            "https://files.example/watch?t=120&r=abc&i=3",
            "https://shop.example/a&b/c.zip?r=0123456789abcdef0123",
            "https://files.example/d/0123456789abcdef0123456789abcdef",
            "https://files.example/api?t=search&q=id",
        ] {
            assert!(!newznab_form(&url(plain)), "{plain}");
        }
    }

    #[test]
    fn an_address_of_a_configured_indexer_is_one_of_its_links() {
        let configured = [
            url("https://api.indexer.example/api"),
            url("http://192.168.1.5:5076/api"),
        ];
        let hosts = IndexerHosts::of(configured.iter());
        for link in [
            "https://indexer.example/get/abc/KEY",
            "http://www.indexer.example/get/abc/KEY",
            "https://API.indexer.example/get/abc/KEY",
            "http://192.168.1.5:5076/get/abc/KEY",
        ] {
            assert!(hosts.holds(&url(link)), "{link}");
        }
        for other in [
            "https://otherindexer.example/get/abc",
            "https://indexer.example.evil.example/get/abc",
            "http://192.168.1.5:8080/file.mkv",
            "https://files.example/indexer.example/a.mkv",
        ] {
            assert!(!hosts.holds(&url(other)), "{other}");
        }
    }
}
