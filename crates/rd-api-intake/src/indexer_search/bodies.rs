//! The request and answer bodies of an indexer search and a grab.

use super::*;

/// One search over one indexer or all enabled ones.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct IndexerSearchRequest {
    /// The indexers to ask; empty asks every enabled one.
    #[serde(default)]
    pub indexer_ids: Vec<IndexerId>,
    /// Sent as `q`: empty, or at least three characters. `!word` excludes a word, as the
    /// indexer defines it.
    #[serde(default)]
    pub query: Option<String>,
    /// The indexer's own category ids, sent as `cat`; empty uses each indexer's own default.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Sent as `maxage`: only releases posted within this many days.
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Sent as `pw=2`: leave out releases the indexer marks as passworded.
    #[serde(default)]
    pub hide_passworded: bool,
    /// Sent as `pred` (0, 1 or 2 as the indexer defines them).
    #[serde(default)]
    pub pretime: Option<u8>,
    /// Results per indexer on this page, 1-500; 100 when absent.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Where the page starts.
    #[serde(default)]
    pub offset: Option<u32>,
    /// `search` (the default), `tv` (`t=tvsearch`) or `movie` (`t=movie`) (RD-1100-03). The
    /// ids below belong to one type each; an indexer that does not answer the type refuses it in
    /// its own outcome, which `POST /api/v1/indexers/{id}/caps` tells in advance.
    #[serde(default)]
    pub search_type: rd_subscription::IndexerSearchType,
    /// `tv` only: the season, sent as `season`.
    #[serde(default)]
    pub season: Option<u32>,
    /// `tv` only: the episode, sent as `ep`; needs a season.
    #[serde(default)]
    pub episode: Option<u32>,
    /// `tv` only: the series' TVDB id, sent as `tvdbid`.
    #[serde(default)]
    pub tvdb_id: Option<u64>,
    /// `tv` only: the series' TVmaze id, sent as `tvmazeid`.
    #[serde(default)]
    pub tvmaze_id: Option<u64>,
    /// `tv` or `movie`: the IMDb id, `tt0903747` or its digits, sent as `imdbid` without `tt`.
    #[serde(default)]
    pub imdb_id: Option<String>,
    /// `movie` only: the film's TMDb id, sent as `tmdbid`.
    #[serde(default)]
    pub tmdb_id: Option<u64>,
}

/// What a hit is: an NZB or a torrent (RD-1100-03).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexerHitKind {
    Nzb,
    Torrent,
}

/// One hit, as the result list shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct IndexerSearchHit {
    pub indexer_id: IndexerId,
    pub indexer_name: String,
    pub title: String,
    /// The indexer's own id for the release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Where the NZB is fetched from, with the API key replaced by `rdownloader-indexer-key`:
    /// what `POST /api/v1/indexers/grab` takes back. Never the key itself.
    pub download: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// The indexer's category id, e.g. `5040`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grabs: Option<u64>,
    /// Whether the indexer marks the release as passworded.
    pub passworded: bool,
    /// What a detailed result row shows besides the title (RD-190-16), as far as the indexer
    /// sent it: `year`, `genre`, `imdbscore`, `language`, `resolution` and `description` (at
    /// most 300 characters). Empty when it sent none of them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// The cover, only ever an absolute http or https address that does not carry the API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(format = "uri")]
    pub cover_url: Option<String>,
    /// `nzb` from a Newznab indexer, `torrent` from a Torznab one (Jackett, Prowlarr), read off
    /// the hit itself (RD-1100-03).
    pub kind: IndexerHitKind,
    /// A torrent's seeders, when the indexer says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u64>,
    /// A torrent's leechers, when the indexer says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leechers: Option<u64>,
    /// The magnet the indexer offers for a torrent, with the API key replaced like `download`'s;
    /// what the grab takes when the download itself cannot be fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
}

/// How one indexer answered.
#[derive(Serialize, ToSchema)]
pub struct IndexerSearchOutcome {
    pub indexer_id: IndexerId,
    pub indexer_name: String,
    /// Hits it returned on this page.
    pub returned: u32,
    /// The total it reports, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Whether it filled the page, so a next page may hold more.
    pub more: bool,
    /// Why it answered nothing: a stable code with its parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MessageResponse>,
}

/// A search's answer: every indexer's hits together, and how each indexer fared.
#[derive(Serialize, ToSchema)]
pub struct IndexerSearchResponse {
    pub hits: Vec<IndexerSearchHit>,
    /// One entry per indexer asked, in the order they were asked.
    pub indexers: Vec<IndexerSearchOutcome>,
}

/// Hits to fetch and put into the LinkGrabber: an NZB as an NZB import, a torrent as a
/// LinkGrabber package.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IndexerGrabRequest {
    /// At most 50.
    pub items: Vec<IndexerGrabItem>,
    /// The category the imports and torrents go to; the routing rules decide when absent.
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}

/// One hit, as the search returned it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IndexerGrabItem {
    pub indexer_id: IndexerId,
    /// The hit's `download`, unchanged.
    pub download: String,
    /// The hit's title, which names the import.
    pub title: String,
    /// The hit's `magnet`, unchanged, when it has one (RD-1100-03): taken when `download` is not
    /// to be had -- Prowlarr answers some downloads with a redirect to the magnet.
    #[serde(default)]
    pub magnet: Option<String>,
}

/// One hit that did not become an import.
#[derive(Serialize, ToSchema)]
pub struct IndexerGrabFailure {
    pub title: String,
    pub error: MessageResponse,
}

/// What a grab became.
#[derive(Serialize, ToSchema)]
pub struct IndexerGrabResponse {
    pub imports: Vec<rd_core::NzbImport>,
    /// The LinkGrabber packages the torrent hits became, one per hit (RD-1100-03).
    pub torrents: Vec<rd_core::CollectorPackage>,
    pub failed: Vec<IndexerGrabFailure>,
}
