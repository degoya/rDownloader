//! The cache question of the online check (RD-130-11): which provider holds a link's file
//! ready right now.
//!
//! Asked of the installed `remote-job` plugins that name a cache kind, **before** the links
//! are checked, and merged into each result as it is written. The host decides what a link
//! is -- a magnet, an indexer's NZB, a hoster's file -- because an address alone cannot say
//! it; the plugin derives its own key from it. A cache answer only ever raises a result: it
//! never turns a link offline, never changes its provider and never its route.

use std::collections::HashMap;

use rd_core::{
    Account, ByteCount, CandidateId, LinkCandidate, LinkCheckResult, LinkStatus,
    TorrentCandidateState,
};
use rd_plugin_ext::{CacheKind, CacheQuery, CacheState, RemoteJobRunners};
use rd_plugin_host::extension::RemoteJobSource;

use crate::remote_job_service::RemoteJobService;

/// Most queries one provider is asked per check run: five calls of a hundred. What is left
/// over stays unasked and is checked exactly as it would have been without a cache.
pub(crate) const MAX_CACHE_QUERIES_PER_RUN: usize = 500;

/// What one provider's cache said about one link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CacheHint {
    /// The provider slug of the account that was asked.
    pub(crate) provider: String,
    pub(crate) state: CacheState,
    pub(crate) file_name: Option<String>,
    pub(crate) size: Option<u64>,
}

/// The cache hints for the candidates of one check run, keyed by candidate.
///
/// Empty -- without compiling a single plugin -- when no enabled account belongs to a
/// provider a `remote-job` manifest claims, which is every installation without such an
/// account. A failure anywhere is logged and costs the hints, never the check.
pub(crate) async fn hints(
    checkers: &RemoteJobService,
    database: &rd_db::Database,
    candidates: &[LinkCandidate],
    accounts: &[Account],
) -> HashMap<CandidateId, CacheHint> {
    let claimed = match checkers.providers().await {
        Ok(claimed) => claimed,
        Err(error) => {
            tracing::warn!(%error, "cache check skipped: the remote-job manifests could not be read");
            return HashMap::new();
        }
    };
    if !accounts
        .iter()
        .any(|account| claimed.contains(&account.provider.to_ascii_lowercase()))
    {
        return HashMap::new();
    }
    let mut queries = Vec::new();
    for candidate in candidates {
        // A stored `.torrent` is asked about by its info hash, which only the torrent state
        // knows; every other link answers from its address alone.
        let torrent = if needs_torrent_state(candidate) {
            database
                .candidate_torrent_state(candidate.id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if let Some(query) = cache_query(candidate, torrent.as_ref()) {
            queries.push((candidate.id, query));
        }
    }
    if queries.is_empty() {
        return HashMap::new();
    }
    let runners = match checkers.runners().await {
        Ok(runners) => runners,
        Err(error) => {
            tracing::warn!(%error, "cache check skipped: the remote-job plugins could not be loaded");
            return HashMap::new();
        }
    };
    ask(&runners, accounts, &queries).await
}

fn needs_torrent_state(candidate: &LinkCandidate) -> bool {
    candidate.provider.as_deref() == Some(rd_core::TORRENT_PROVIDER)
        && candidate.url.scheme() == "file"
}

/// What a candidate is asked as, if it is asked at all.
///
/// | provider | address | query |
/// | --- | --- | --- |
/// | `torrent` | `magnet:` | the magnet, `torrent` |
/// | `torrent` | `file:` with a known info hash | the magnet of that hash, `torrent` |
/// | `nzb` | `http(s)` | the address, `usenet` |
/// | a hoster's slug | `http(s)` | the address, `hoster` |
///
/// Nothing else: a `.torrent` behind an address would have to be downloaded first, and a
/// direct link, a media page, a gallery, a recording or a file server has no provider cache
/// to ask.
pub(crate) fn cache_query(
    candidate: &LinkCandidate,
    torrent: Option<&TorrentCandidateState>,
) -> Option<CacheQuery> {
    let url = &candidate.url;
    let http = matches!(url.scheme(), "http" | "https");
    match candidate.provider.as_deref().unwrap_or("direct_http") {
        rd_core::TORRENT_PROVIDER => match url.scheme() {
            "magnet" => Some(CacheQuery {
                source: RemoteJobSource::Magnet(url.as_str().to_owned()),
                kind: CacheKind::Torrent,
            }),
            "file" => torrent
                .and_then(|state| state.metadata.as_ref())
                .map(|metadata| metadata.info_hash.as_str())
                .filter(|hash| is_info_hash(hash))
                .map(|hash| CacheQuery {
                    source: RemoteJobSource::Magnet(format!(
                        "magnet:?xt=urn:btih:{}",
                        hash.to_ascii_lowercase()
                    )),
                    kind: CacheKind::Torrent,
                }),
            _ => None,
        },
        rd_core::NZB_PROVIDER => http.then(|| CacheQuery {
            source: RemoteJobSource::Address(url.as_str().to_owned()),
            kind: CacheKind::Usenet,
        }),
        "direct_http"
        | rd_core::MEDIA_PROVIDER
        | rd_core::GALLERY_PROVIDER
        | rd_core::RECORD_PROVIDER
        | rd_core::FTP_PROVIDER
        | rd_core::SFTP_PROVIDER
        | rd_core::WEBDAV_PROVIDER => None,
        _ => http.then(|| CacheQuery {
            source: RemoteJobSource::Address(url.as_str().to_owned()),
            kind: CacheKind::Hoster,
        }),
    }
}

/// A v1 info hash as hex: forty digits and nothing else.
fn is_info_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Asks every provider that has a cache kind and an enabled account.
///
/// One account per provider -- the first enabled one in the order the accounts are listed --
/// because a cache belongs to the provider, not to the account. Providers are asked one
/// after another. Where two answer for one link, `cached` beats `known`, and on a tie the
/// alphabetically first slug wins; `unknown` is never kept. A provider that refuses loses
/// its answers and nothing else: no link gets a message or a state from it.
pub(crate) async fn ask(
    runners: &RemoteJobRunners,
    accounts: &[Account],
    queries: &[(CandidateId, CacheQuery)],
) -> HashMap<CandidateId, CacheHint> {
    let mut hints: HashMap<CandidateId, CacheHint> = HashMap::new();
    for (slug, kinds) in runners.cache_providers().await {
        let Some(account) = accounts
            .iter()
            .find(|account| account.enabled && account.provider.eq_ignore_ascii_case(&slug))
        else {
            continue;
        };
        let asked: Vec<&(CandidateId, CacheQuery)> = queries
            .iter()
            .filter(|(_, query)| kinds.contains(&query.kind))
            .take(MAX_CACHE_QUERIES_PER_RUN)
            .collect();
        if asked.is_empty() {
            continue;
        }
        let batch: Vec<CacheQuery> = asked.iter().map(|(_, query)| query.clone()).collect();
        let answers = match runners.check_cached(&slug, account.id, &batch).await {
            Ok(answers) => answers,
            Err(refusal) => {
                tracing::warn!(
                    provider = %slug,
                    code = %refusal.code,
                    "cache check refused; its answers are dropped"
                );
                continue;
            }
        };
        for ((id, _), answer) in asked.into_iter().zip(answers) {
            if answer.state == CacheState::Unknown {
                continue;
            }
            let hint = CacheHint {
                provider: slug.clone(),
                state: answer.state,
                file_name: answer.file_name,
                size: answer.size,
            };
            let keep = hints
                .get(id)
                .is_none_or(|existing| outranks(&hint, existing));
            if keep {
                hints.insert(*id, hint);
            }
        }
    }
    hints
}

fn rank(state: CacheState) -> u8 {
    match state {
        CacheState::Cached => 2,
        CacheState::Known => 1,
        CacheState::Unknown => 0,
    }
}

fn outranks(candidate: &CacheHint, existing: &CacheHint) -> bool {
    match rank(candidate.state).cmp(&rank(existing.state)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => candidate.provider < existing.provider,
    }
}

/// Merges a cache hint into the result of a check, and names the provider that answered.
///
/// `checked_by` is the slug of the account whose resolver checked the link, when one did. A
/// resolver that answered `cached` itself wins over any hint, because its account is the one
/// that will also download the file.
///
/// - no result (the check failed), `offline` or `unresolvable`: unchanged, nobody named;
/// - a `cached` hint on `online`, `unknown` or `cached`: `cached`, with the hint's name and
///   size where the check had none, and the hint's provider named;
/// - a `known` hint on `unknown`: `online` -- the provider knows the file, which is more than
///   the check could say -- and nobody named;
/// - anything else: unchanged.
pub(crate) fn merge(
    result: Option<LinkCheckResult>,
    hint: Option<&CacheHint>,
    checked_by: Option<&str>,
) -> (Option<LinkCheckResult>, Option<String>) {
    let Some(mut result) = result else {
        return (None, None);
    };
    match result.status {
        LinkStatus::Offline | LinkStatus::Unresolvable => return (Some(result), None),
        LinkStatus::Cached => {
            if let Some(slug) = checked_by {
                return (Some(result), Some(slug.to_owned()));
            }
        }
        LinkStatus::Online | LinkStatus::Unknown => {}
    }
    match hint {
        Some(hint) if hint.state == CacheState::Cached => {
            result.status = LinkStatus::Cached;
            if result.file_name.is_none() {
                result.file_name.clone_from(&hint.file_name);
            }
            if result.size.is_none() {
                result.size = hint.size.and_then(|size| ByteCount::new(size).ok());
            }
            (Some(result), Some(hint.provider.clone()))
        }
        Some(hint) if hint.state == CacheState::Known && result.status == LinkStatus::Unknown => {
            result.status = LinkStatus::Online;
            (Some(result), None)
        }
        _ => (Some(result), None),
    }
}

/// The provider to stamp on a link nothing here can check: only a `cached` answer counts.
pub(crate) fn unsupported_cached_by(hint: Option<&CacheHint>) -> Option<String> {
    hint.filter(|hint| hint.state == CacheState::Cached)
        .map(|hint| hint.provider.clone())
}

#[cfg(test)]
#[path = "link_check_cache_tests.rs"]
mod tests;
