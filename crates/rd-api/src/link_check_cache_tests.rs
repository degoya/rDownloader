//! The cache question of the online check (RD-130-11), against stand-in providers.
//!
//! The provider answers at the adapter boundary, as in the remote-job sweep's own tests: no
//! component is compiled, and what is proven is which link is asked as what, how answers of
//! two providers are ranked, and what a hint does to a result when it is written.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{
    Account, AccountId, ByteCount, CandidateId, IngressSource, LinkCandidate, LinkCandidateState,
    LinkCheckResult, LinkStatus, TorrentCandidateState,
};
use rd_plugin_ext::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, RemoteJobDriver, RemoteJobRunners, RunnerInfo,
};
use rd_plugin_host::extension::{
    RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource,
};

use super::{CacheHint, MAX_CACHE_QUERIES_PER_RUN, ask, cache_query, merge, unsupported_cached_by};

const MAGNET: &str = "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=held";
const HASH: &str = "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709";

/// A provider's cache: the kinds it names, what it answers for a source, and every batch it
/// was handed.
struct Cache {
    kinds: Vec<CacheKind>,
    /// The answer for a source whose text contains `held`; everything else is `Unknown`.
    held: CacheState,
    refuses: bool,
    asked: Mutex<Vec<usize>>,
}

impl Cache {
    fn new(kinds: &[CacheKind], held: CacheState) -> Arc<Self> {
        Arc::new(Self {
            kinds: kinds.to_vec(),
            held,
            refuses: false,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn refusing(kinds: &[CacheKind]) -> Arc<Self> {
        Arc::new(Self {
            kinds: kinds.to_vec(),
            held: CacheState::Cached,
            refuses: true,
            asked: Mutex::new(Vec::new()),
        })
    }
}

struct Driver(Arc<Cache>);

#[async_trait]
impl RemoteJobDriver for Driver {
    async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
        Ok(true)
    }

    async fn identify(
        &self,
        _source: &RemoteJobSource,
    ) -> Result<Result<String, RemoteJobRefusal>> {
        Ok(Ok("key".to_owned()))
    }

    async fn submit(
        &self,
        _account: AccountId,
        _source: &RemoteJobSource,
        _content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        anyhow::bail!("a cache check never submits")
    }

    async fn adopt(
        &self,
        _account: AccountId,
        _content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        Ok(Ok(None))
    }

    async fn poll(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        anyhow::bail!("a cache check never polls")
    }

    async fn choose(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
        _chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }

    async fn discard(
        &self,
        _account: AccountId,
        _handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Ok(Ok(()))
    }

    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        Ok(self.0.kinds.clone())
    }

    async fn check_cached(
        &self,
        _account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        self.0.asked.lock().expect("asked").push(queries.len());
        if self.0.refuses {
            return Ok(Err(RemoteJobRefusal {
                code: Some("torbox_jobs.rate_limited".to_owned()),
                message: "slow down".to_owned(),
                category: rd_core::FailureKind::RateLimited {
                    retry_after_seconds: None,
                },
            }));
        }
        Ok(Ok(queries
            .iter()
            .map(|query| {
                let text = match &query.source {
                    RemoteJobSource::Magnet(text) | RemoteJobSource::Address(text) => text.as_str(),
                    RemoteJobSource::Container(_) => "",
                };
                if text.contains("held") {
                    CacheAnswer {
                        state: self.0.held,
                        file_name: Some("Held.Release.mkv".to_owned()),
                        size: Some(4_096),
                    }
                } else {
                    CacheAnswer::unknown()
                }
            })
            .collect()))
    }
}

fn runners(providers: &[(&str, &Arc<Cache>)]) -> RemoteJobRunners {
    RemoteJobRunners::from_drivers(
        providers
            .iter()
            .enumerate()
            .map(|(index, (slug, cache))| {
                (
                    RunnerInfo {
                        plugin_id: format!("019d0000-0000-7000-8000-0000000001{index:02}"),
                        name: format!("{slug} jobs"),
                        claims: vec![(*slug).to_owned()],
                    },
                    Box::new(Driver(Arc::clone(cache))) as Box<dyn RemoteJobDriver>,
                )
            })
            .collect(),
    )
}

fn account(provider: &str) -> Account {
    Account {
        id: AccountId::new(),
        provider: provider.to_owned(),
        label: provider.to_owned(),
        username: None,
        credential_mode: None,
        proxy_profile_id: None,
        enabled: true,
        has_secret: true,
        has_cookies: false,
    }
}

fn candidate(url: &str, provider: Option<&str>) -> LinkCandidate {
    serde_json::from_value(serde_json::json!({
        "id": CandidateId::new(),
        "batch_id": rd_core::BatchId::new(),
        "url": url,
        "state": "checking",
        "file_name": null,
        "size": null,
        "provider": provider,
        "category_id": null,
        "route": null,
        "error": null,
        "error_code": null,
        "package_id": null,
        "checked_at": null,
        "created_at": chrono::Utc::now(),
    }))
    .expect("a candidate")
}

fn result(status: LinkStatus) -> LinkCheckResult {
    LinkCheckResult {
        url: "https://rapidgator.net/file/abc".parse().expect("URL"),
        status,
        file_name: None,
        size: None,
        media: None,
    }
}

fn hint(provider: &str, state: CacheState) -> CacheHint {
    CacheHint {
        provider: provider.to_owned(),
        state,
        file_name: Some("Held.Release.mkv".to_owned()),
        size: Some(4_096),
    }
}

fn torrent_state(hash: &str) -> TorrentCandidateState {
    TorrentCandidateState {
        metadata: Some(
            serde_json::from_value(serde_json::json!({
            "info_hash": hash,
            "name": "release",
            "total_bytes": "1",
            "piece_length": 16_384,
            "piece_count": 1,
            "private": false,
            "files": [],
            "trackers": [],
            "web_seeds": [],
            }))
            .expect("metadata"),
        ),
        ..TorrentCandidateState::default()
    }
}

/// Every row of the table in `cache_query`, and the rows that ask nothing.
#[test]
fn each_link_is_asked_as_what_the_host_knows_it_to_be() {
    let magnet = cache_query(&candidate(MAGNET, Some("torrent")), None).expect("a magnet");
    assert_eq!(magnet.kind, CacheKind::Torrent);
    assert_eq!(magnet.source, RemoteJobSource::Magnet(MAGNET.to_owned()));

    // A stored `.torrent` is asked about by the magnet of its hash, never by its bytes.
    let stored = candidate(
        "file:///var/lib/rdownloader/torrents/a.torrent",
        Some("torrent"),
    );
    let asked = cache_query(&stored, Some(&torrent_state(HASH))).expect("a stored torrent");
    assert_eq!(
        asked.source,
        RemoteJobSource::Magnet(format!("magnet:?xt=urn:btih:{}", HASH.to_ascii_lowercase()))
    );
    assert_eq!(asked.kind, CacheKind::Torrent);
    assert_eq!(
        cache_query(&stored, None),
        None,
        "no hash known, nothing asked"
    );
    assert_eq!(
        cache_query(&stored, Some(&torrent_state("not-a-hash"))),
        None
    );
    // A `.torrent` behind an address would have to be downloaded to be asked about.
    assert_eq!(
        cache_query(
            &candidate("https://tracker.example/t/1.torrent", Some("torrent")),
            None
        ),
        None
    );

    let nzb = cache_query(
        &candidate("https://indexer.example/api?t=get&id=1", Some("nzb")),
        None,
    )
    .expect("an NZB link");
    assert_eq!(nzb.kind, CacheKind::Usenet);
    assert_eq!(
        nzb.source,
        RemoteJobSource::Address("https://indexer.example/api?t=get&id=1".to_owned())
    );

    let hoster = cache_query(
        &candidate("https://rapidgator.net/file/abc", Some("rapidgator")),
        None,
    )
    .expect("a hoster link");
    assert_eq!(hoster.kind, CacheKind::Hoster);

    for (url, provider) in [
        ("https://cdn.example/file.bin", Some("direct_http")),
        ("https://cdn.example/file.bin", None),
        ("https://www.youtube.com/watch?v=1", Some("media")),
        ("https://gallery.example/album/1", Some("gallery")),
        ("https://live.example/stream.m3u8", Some("record")),
        ("ftp://files.example/a.bin", Some("ftp")),
        ("sftp://files.example/a.bin", Some("sftp")),
        ("https://dav.example/a.bin", Some("webdav")),
    ] {
        assert_eq!(
            cache_query(&candidate(url, provider), None),
            None,
            "{url} as {provider:?} has no provider cache to ask"
        );
    }
}

/// Each rule of `merge`, above all: a cache never turns a link offline, and `known` only
/// raises `unknown`.
#[test]
fn a_cache_answer_only_ever_raises_a_result() {
    let cached = hint("torbox", CacheState::Cached);
    let known = hint("premiumize", CacheState::Known);

    let (merged, by) = merge(Some(result(LinkStatus::Offline)), Some(&cached), None);
    assert_eq!(merged.expect("result").status, LinkStatus::Offline);
    assert_eq!(by, None);
    let (merged, by) = merge(Some(result(LinkStatus::Unresolvable)), Some(&cached), None);
    assert_eq!(merged.expect("result").status, LinkStatus::Unresolvable);
    assert_eq!(by, None);
    let (merged, by) = merge(None, Some(&cached), None);
    assert!(merged.is_none());
    assert_eq!(by, None);

    for status in [LinkStatus::Online, LinkStatus::Unknown, LinkStatus::Cached] {
        let (merged, by) = merge(Some(result(status)), Some(&cached), None);
        let merged = merged.expect("result");
        assert_eq!(merged.status, LinkStatus::Cached);
        assert_eq!(merged.file_name.as_deref(), Some("Held.Release.mkv"));
        assert_eq!(merged.size, Some(ByteCount::new(4_096).expect("size")));
        assert_eq!(by.as_deref(), Some("torbox"));
    }
    // What the check learned itself is kept.
    let mut named = result(LinkStatus::Online);
    named.file_name = Some("Own.Name.mkv".to_owned());
    let (merged, _) = merge(Some(named), Some(&cached), None);
    assert_eq!(
        merged.expect("result").file_name.as_deref(),
        Some("Own.Name.mkv")
    );

    let (merged, by) = merge(Some(result(LinkStatus::Unknown)), Some(&known), None);
    assert_eq!(merged.expect("result").status, LinkStatus::Online);
    assert_eq!(by, None);
    let (merged, by) = merge(Some(result(LinkStatus::Online)), Some(&known), None);
    assert_eq!(merged.expect("result").status, LinkStatus::Online);
    assert_eq!(by, None);

    // The resolver that checked the link answered `cached` itself: its own account wins.
    let (merged, by) = merge(
        Some(result(LinkStatus::Cached)),
        Some(&cached),
        Some("premiumize"),
    );
    assert_eq!(merged.expect("result").status, LinkStatus::Cached);
    assert_eq!(by.as_deref(), Some("premiumize"));
    // No hint and no cache answer: nothing changes, nobody is named.
    let (merged, by) = merge(Some(result(LinkStatus::Online)), None, Some("premiumize"));
    assert_eq!(merged.expect("result").status, LinkStatus::Online);
    assert_eq!(by, None);

    assert_eq!(
        unsupported_cached_by(Some(&cached)).as_deref(),
        Some("torbox")
    );
    assert_eq!(unsupported_cached_by(Some(&known)), None);
    assert_eq!(unsupported_cached_by(None), None);
}

/// Two providers answer for one link: `cached` beats `known`, and a tie goes to the slug
/// that sorts first.
#[tokio::test]
async fn cached_beats_known_and_a_tie_goes_to_the_first_slug() {
    let torbox = Cache::new(&[CacheKind::Torrent], CacheState::Known);
    let premiumize = Cache::new(&[CacheKind::Torrent], CacheState::Cached);
    let installed = runners(&[("torbox", &torbox), ("premiumize", &premiumize)]);
    let id = CandidateId::new();
    let queries = vec![(
        id,
        cache_query(&candidate(MAGNET, Some("torrent")), None).expect("query"),
    )];
    let accounts = [account("torbox"), account("premiumize")];
    let hints = ask(&installed, &accounts, &queries).await;
    assert_eq!(hints[&id].provider, "premiumize");
    assert_eq!(hints[&id].state, CacheState::Cached);

    let torbox = Cache::new(&[CacheKind::Torrent], CacheState::Cached);
    let premiumize = Cache::new(&[CacheKind::Torrent], CacheState::Cached);
    let installed = runners(&[("torbox", &torbox), ("premiumize", &premiumize)]);
    let hints = ask(&installed, &accounts, &queries).await;
    assert_eq!(hints[&id].provider, "premiumize");
}

/// Only a provider with an enabled account is asked, only for its own kinds, only up to the
/// per-run ceiling -- and an `unknown` is never kept as a hint.
#[tokio::test]
async fn only_a_provider_with_an_account_is_asked_and_only_for_its_kinds() {
    let torbox = Cache::new(&[CacheKind::Hoster], CacheState::Cached);
    let installed = runners(&[("torbox", &torbox)]);
    let magnet = (
        CandidateId::new(),
        cache_query(&candidate(MAGNET, Some("torrent")), None).expect("query"),
    );
    let hoster = (
        CandidateId::new(),
        cache_query(
            &candidate("https://rapidgator.net/file/held", Some("rapidgator")),
            None,
        )
        .expect("query"),
    );
    let quiet = (
        CandidateId::new(),
        cache_query(
            &candidate("https://rapidgator.net/file/other", Some("rapidgator")),
            None,
        )
        .expect("query"),
    );
    let queries = vec![magnet.clone(), hoster.clone(), quiet.clone()];

    // No account of the provider: nobody is asked.
    assert!(
        ask(&installed, &[account("realdebrid")], &queries)
            .await
            .is_empty()
    );
    let mut disabled = account("torbox");
    disabled.enabled = false;
    assert!(ask(&installed, &[disabled], &queries).await.is_empty());
    assert!(torbox.asked.lock().expect("asked").is_empty());

    let hints = ask(&installed, &[account("TorBox")], &queries).await;
    assert_eq!(
        *torbox.asked.lock().expect("asked"),
        vec![2],
        "the magnet stays home"
    );
    assert_eq!(hints.len(), 1, "an unknown is no hint");
    assert_eq!(hints[&hoster.0].provider, "torbox");

    // A run with more links than the ceiling asks the ceiling and leaves the rest alone.
    let many: Vec<(CandidateId, CacheQuery)> = (0..MAX_CACHE_QUERIES_PER_RUN + 20)
        .map(|index| {
            (
                CandidateId::new(),
                cache_query(
                    &candidate(
                        &format!("https://rapidgator.net/file/held-{index}"),
                        Some("rapidgator"),
                    ),
                    None,
                )
                .expect("query"),
            )
        })
        .collect();
    let counted = Cache::new(&[CacheKind::Hoster], CacheState::Cached);
    let hints = ask(
        &runners(&[("torbox", &counted)]),
        &[account("torbox")],
        &many,
    )
    .await;
    assert_eq!(hints.len(), MAX_CACHE_QUERIES_PER_RUN);
    assert_eq!(
        counted.asked.lock().expect("asked").iter().sum::<usize>(),
        MAX_CACHE_QUERIES_PER_RUN
    );
}

/// The whole way into the database: a magnet without a cache provider stays `online` with no
/// stamp, one the provider holds is stamped with the provider, a hoster link nothing here can
/// check stays `unsupported` and still carries the stamp, and a refusing provider changes
/// nothing at all.
#[tokio::test]
async fn a_cache_answer_reaches_the_candidate_and_a_refusal_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("cache.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        MAGNET,
        "magnet:?xt=urn:btih:0000000000000000000000000000000000000001",
        "https://rapidgator.net/file/held",
    ]
    .iter()
    .map(|url| url.parse().expect("URL"))
    .collect();
    let (_batch, _packages, candidates) = database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![
                Some("torrent".to_owned()),
                Some("torrent".to_owned()),
                Some("rapidgator".to_owned()),
            ],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    // What `process` does with the three: the magnets pass unprobed as `online`, the hoster
    // link has no account and is marked unsupported.
    let write = |hints: std::collections::HashMap<CandidateId, CacheHint>| {
        let database = database.clone();
        let candidates = candidates.clone();
        async move {
            database
                .claim_candidates_for_check(candidates.iter().map(|c| c.id).collect())
                .await
                .expect("claim");
            for candidate in &candidates[..2] {
                let online = LinkCheckResult {
                    url: candidate.url.clone(),
                    status: LinkStatus::Online,
                    file_name: None,
                    size: None,
                    media: None,
                };
                let (merged, cached_by) = merge(Some(online), hints.get(&candidate.id), None);
                database
                    .record_candidate_check(candidate.id, merged, None, false, cached_by)
                    .await
                    .expect("record");
            }
            database
                .mark_candidate_unsupported(
                    candidates[2].id,
                    rd_core::CandidateMessage::coded(
                        "collector.check_no_source",
                        "No check source for this hoster (account missing)",
                    ),
                    unsupported_cached_by(hints.get(&candidates[2].id)),
                )
                .await
                .expect("unsupported");
            let mut rows = Vec::new();
            for candidate in &candidates {
                rows.push(
                    database
                        .get_candidate(candidate.id)
                        .await
                        .expect("read")
                        .expect("row"),
                );
            }
            rows
        }
    };
    let queries: Vec<(CandidateId, CacheQuery)> = candidates
        .iter()
        .filter_map(|candidate| cache_query(candidate, None).map(|query| (candidate.id, query)))
        .collect();
    assert_eq!(queries.len(), 3);

    let torbox = Cache::new(&[CacheKind::Torrent, CacheKind::Hoster], CacheState::Cached);
    let hints = ask(
        &runners(&[("torbox", &torbox)]),
        &[account("torbox")],
        &queries,
    )
    .await;
    let rows = write(hints).await;
    assert_eq!(rows[0].state, LinkCandidateState::Online);
    assert!(rows[0].cached_at.is_some());
    assert_eq!(rows[0].cached_by.as_deref(), Some("torbox"));
    assert_eq!(rows[0].file_name.as_deref(), Some("Held.Release.mkv"));
    assert_eq!(rows[1].state, LinkCandidateState::Online);
    assert_eq!(rows[1].cached_at, None, "not held, not stamped");
    assert_eq!(rows[1].cached_by, None);
    assert_eq!(rows[2].state, LinkCandidateState::Unsupported);
    assert_eq!(
        rows[2].error_code.as_deref(),
        Some("collector.check_no_source")
    );
    assert!(rows[2].cached_at.is_some());
    assert_eq!(rows[2].cached_by.as_deref(), Some("torbox"));

    // A provider that refuses: every link is written exactly as without a cache provider.
    let refusing = Cache::refusing(&[CacheKind::Torrent, CacheKind::Hoster]);
    let hints = ask(
        &runners(&[("torbox", &refusing)]),
        &[account("torbox")],
        &queries,
    )
    .await;
    assert!(hints.is_empty());
    let rows = write(hints).await;
    for row in &rows {
        assert_eq!(row.cached_at, None);
        assert_eq!(row.cached_by, None);
    }
    assert_eq!(rows[0].state, LinkCandidateState::Online);
    assert_eq!(
        rows[0].error_code, None,
        "a refusal leaves no message behind"
    );
    assert_eq!(rows[2].state, LinkCandidateState::Unsupported);
}
