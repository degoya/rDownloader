//! MCP tools for the defined Newznab and Torznab indexers: listing them, searching them -- with a
//! TV or film search's ids too (RD-1100-03) -- and taking hits into the LinkGrabber (RD-180-19).
//!
//! Defining, editing and testing an indexer stays out -- it takes an API key in, which is one of
//! the owner's four marks (see the coverage table in `rd-api/src/mcp_coverage.rs`). Searching and
//! grabbing meet none of them: the key is used, never shown -- a hit's download address carries
//! a placeholder where the key stands -- and a grab ends as an NZB import or a LinkGrabber
//! package waiting for review, exactly like `import_nzb` or a pasted magnet.

use axum::{Json, extract::State};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, parse_ids, respond},
};
use crate::{indexer_handlers, indexer_search};

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IndexerSearchParams {
    /// Indexer ids from list_indexers; empty or absent searches every enabled indexer.
    #[serde(default)]
    pub indexer_ids: Vec<String>,
    /// The search term: empty, or at least three characters. `!word` excludes a word.
    #[serde(default)]
    pub query: Option<String>,
    /// The indexer's own category ids (e.g. `5040`); absent uses each indexer's defaults.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Only releases posted within this many days.
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Leave out releases the indexer marks as passworded.
    #[serde(default)]
    pub hide_passworded: bool,
    /// Results per indexer, 1-500 (default 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Where the page starts; the next page is offset + limit.
    #[serde(default)]
    pub offset: Option<u32>,
    /// `search` (default), `tv` or `movie`. An indexer that does not answer the type refuses it
    /// in its own outcome.
    #[serde(default)]
    pub search_type: SearchType,
    /// tv: the season.
    #[serde(default)]
    pub season: Option<u32>,
    /// tv: the episode; needs season.
    #[serde(default)]
    pub episode: Option<u32>,
    /// tv: the series' TVDB id.
    #[serde(default)]
    pub tvdb_id: Option<u64>,
    /// tv: the series' TVmaze id.
    #[serde(default)]
    pub tvmaze_id: Option<u64>,
    /// tv or movie: the IMDb id, e.g. `tt0903747`.
    #[serde(default)]
    pub imdb_id: Option<String>,
    /// movie: the film's TMDb id.
    #[serde(default)]
    pub tmdb_id: Option<u64>,
}

/// The search function: `t=search`, `t=tvsearch` or `t=movie`.
#[derive(Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchType {
    #[default]
    Search,
    Tv,
    Movie,
}

impl From<SearchType> for indexer_search::IndexerSearchType {
    fn from(value: SearchType) -> Self {
        match value {
            SearchType::Search => Self::Search,
            SearchType::Tv => Self::Tv,
            SearchType::Movie => Self::Movie,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IndexerGrabHit {
    /// The hit's indexer_id, as search_indexers returned it.
    pub indexer_id: String,
    /// The hit's download, unchanged.
    pub download: String,
    /// The hit's title.
    pub title: String,
    /// The hit's magnet, unchanged, when it has one.
    #[serde(default)]
    pub magnet: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct IndexerGrabParams {
    /// Hits from search_indexers, at most 50.
    pub items: Vec<IndexerGrabHit>,
    /// The category the NZB imports and torrents go to (id from list_configuration); absent
    /// lets the routing rules decide.
    #[serde(default)]
    pub category_id: Option<String>,
}

#[tool_router(router = indexers_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the Newznab and Torznab (Jackett, Prowlarr) indexers defined in the web UI: id, name, address, default categories, whether it is enabled, whether a key is stored, and its list_style (compact or detailed: how the web UI draws its hits). The key itself is never included; indexers are defined and tested in the web UI only."
    )]
    pub async fn list_indexers(&self) -> McpToolResult {
        respond(
            indexer_handlers::list_indexers(State(self.state.clone()))
                .await
                .map(|Json(rows)| rows),
        )
    }

    #[tool(
        description = "Search one or every enabled indexer, one request per indexer and page (indexers cache and count requests, so do not repeat a search to wait for news). search_type tv sends t=tvsearch with season, episode, tvdb_id, tvmaze_id, imdb_id; movie sends t=movie with imdb_id, tmdb_id; an id of another type is refused as indexer.search_field_unsupported, and an indexer that does not answer the type refuses it in its outcome. Answers the hits (title, kind nzb or torrent, size_bytes, published_at, category, grabs, passworded, seeders and leechers and magnet for a torrent, indexer, download, and when the indexer sent them metadata -- year, genre, imdbscore, language, resolution, description -- and cover_url) and per indexer how many came, whether a next page may hold more, and a coded error when it refused (indexer.credentials_refused, indexer.query_rejected, indexer.limit_reached, ...). A term of one or two characters is refused as indexer.query_too_short."
    )]
    pub async fn search_indexers(
        &self,
        Parameters(params): Parameters<IndexerSearchParams>,
    ) -> McpToolResult {
        let result = async {
            let request = indexer_search::IndexerSearchRequest {
                indexer_ids: parse_ids(&params.indexer_ids)?,
                query: params.query,
                categories: params.categories,
                max_age_days: params.max_age_days,
                hide_passworded: params.hide_passworded,
                pretime: None,
                limit: params.limit,
                offset: params.offset,
                search_type: params.search_type.into(),
                season: params.season,
                episode: params.episode,
                tvdb_id: params.tvdb_id,
                tvmaze_id: params.tvmaze_id,
                imdb_id: params.imdb_id,
                tmdb_id: params.tmdb_id,
            };
            let Json(answer) =
                indexer_search::search_indexers(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Fetch chosen hits of search_indexers and put each into the LinkGrabber: an NZB as an NZB import waiting for review (list_nzb_imports, enqueue_nzb_import), the same path an uploaded .nzb takes; a torrent as a LinkGrabber package (list_collector), the path an uploaded .torrent or a pasted magnet takes. Pass each hit's indexer_id, download, title and magnet unchanged. Answers the imports, the torrents' packages and, per hit that failed, a coded error."
    )]
    pub async fn grab_indexer_results(
        &self,
        Parameters(params): Parameters<IndexerGrabParams>,
    ) -> McpToolResult {
        let result = async {
            let mut items = Vec::with_capacity(params.items.len());
            for hit in params.items {
                items.push(indexer_search::IndexerGrabItem {
                    indexer_id: parse_id(&hit.indexer_id)?,
                    download: hit.download,
                    title: hit.title,
                    magnet: hit.magnet,
                });
            }
            let category_id = params.category_id.as_deref().map(parse_id).transpose()?;
            let Json(answer) = indexer_search::grab_indexer_results(
                State(self.state.clone()),
                Json(indexer_search::IndexerGrabRequest { items, category_id }),
            )
            .await?;
            // An import row can carry the archive password its NZB named.
            super::params_handling::public(&answer)
        }
        .await;
        respond(result)
    }
}
