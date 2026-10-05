//! The rows the candidate and batch queries read, and their conversion into the domain types.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{CollectorBatch, LinkCandidate};
use sqlx::FromRow;
use url::Url;

use crate::{parse_enum, parse_id};

#[derive(FromRow)]
pub(super) struct BatchRow {
    id: String,
    source: String,
    source_label: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<BatchRow> for CollectorBatch {
    type Error = anyhow::Error;

    fn try_from(row: BatchRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            source: parse_enum(&row.source)?,
            source_label: row.source_label,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
pub(crate) struct CandidateRow {
    id: String,
    batch_id: String,
    url: String,
    state: String,
    file_name: Option<String>,
    size: Option<i64>,
    provider: Option<String>,
    category_id: Option<String>,
    priority: i64,
    route_json: Option<String>,
    error: Option<String>,
    error_code: Option<String>,
    package_id: Option<String>,
    position: i64,
    checked_at: Option<DateTime<Utc>>,
    cached_at: Option<DateTime<Utc>>,
    cached_by: Option<String>,
    created_at: DateTime<Utc>,
    media_json: Option<String>,
    request_json: Option<String>,
    replay_consent_json: Option<String>,
    torrent_json: Option<String>,
    listing_json: Option<String>,
    remote_credential_id: Option<String>,
    auth_profile_id: Option<String>,
    auth_profile_pinned: i64,
    enrichment_json: Option<String>,
    file_name_declared: i64,
    mirror_group: Option<String>,
    mirror_source: Option<String>,
    mirror_selected: i64,
    mirror_pinned: i64,
    mirror_quality: Option<String>,
    mirror_language: Option<String>,
    /// The `vault://` reference of the fragment this link arrived with (RD-110-38); the
    /// candidate struct is told only *that* one exists, never what it is.
    secret_fragment_ref: Option<String>,
    /// The checked source set a Metalink parser stated (RD-150-03), shown before queueing.
    source_set_json: Option<String>,
}

impl TryFrom<CandidateRow> for LinkCandidate {
    type Error = anyhow::Error;

    fn try_from(row: CandidateRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            batch_id: parse_id(&row.batch_id)?,
            url: Url::parse(&row.url)?,
            state: parse_enum(&row.state)?,
            file_name: row.file_name,
            file_name_declared: row.file_name_declared != 0,
            size: row
                .size
                .map(|value| u64::try_from(value).context("negative candidate size"))
                .transpose()?
                .map(rd_core::ByteCount::new)
                .transpose()
                .map_err(anyhow::Error::msg)?,
            provider: row.provider,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            priority: rd_core::DownloadPriority::from_i32(
                i32::try_from(row.priority).unwrap_or_default(),
            ),
            route: row
                .route_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            error: row.error,
            error_code: row.error_code,
            package_id: row.package_id.as_deref().map(parse_id).transpose()?,
            position: row.position,
            checked_at: row.checked_at,
            cached_at: row.cached_at,
            cached_by: row.cached_by,
            created_at: row.created_at,
            media: row
                .media_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("parse media info")?,
            request: row
                .request_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("parse captured request")?,
            replay_consent: row
                .replay_consent_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("parse replay consent")?,
            torrent: row
                .torrent_json
                .as_deref()
                .map(serde_json::from_str::<rd_core::TorrentCandidateState>)
                .transpose()
                .context("parse candidate torrent state")?
                .as_ref()
                .map(rd_core::TorrentCandidateState::summary),
            listing: row
                .listing_json
                .as_deref()
                .map(serde_json::from_str::<rd_core::RemoteCandidateState>)
                .transpose()
                .context("parse candidate remote listing")?
                .as_ref()
                .map(rd_core::RemoteCandidateState::summary),
            remote_credential_id: row
                .remote_credential_id
                .as_deref()
                .map(parse_id)
                .transpose()?,
            auth_profile: rd_core::AuthProfileSelection::from_columns(
                row.auth_profile_id.as_deref().map(parse_id).transpose()?,
                row.auth_profile_pinned != 0,
            ),
            // A malformed blob must not hide the candidate; the link is what matters and the
            // enrichment is an addition to it.
            enrichment: row
                .enrichment_json
                .as_deref()
                .and_then(|value| {
                    crate::json_column::lenient(
                        serde_json::from_str(value),
                        "link_candidates",
                        "enrichment_json",
                        &row.id,
                    )
                })
                .unwrap_or_default(),
            // A group without a source, or the other way round, is a row half-written by
            // nothing that exists: both columns are set together or neither is.
            mirror: match (row.mirror_group, row.mirror_source) {
                (Some(group), Some(source)) => Some(rd_core::CandidateMirror {
                    group,
                    source: parse_enum(&source)?,
                    selected: row.mirror_selected != 0,
                    pinned: row.mirror_pinned != 0,
                    quality: row.mirror_quality,
                    language: row.mirror_language,
                }),
                _ => None,
            },
            secret_fragment: row.secret_fragment_ref.is_some(),
            // Like the enrichment: a set that no longer parses hides its mirrors, never the
            // link, which is queued as the single address it also is.
            sources: row
                .source_set_json
                .as_deref()
                .and_then(|value| {
                    crate::json_column::lenient::<rd_core::SourceSet>(
                        serde_json::from_str(value),
                        "link_candidates",
                        "source_set_json",
                        &row.id,
                    )
                })
                .map(|set| set.preview())
                .unwrap_or_default(),
        })
    }
}

/// The columns a `CandidateRow` reads, once for both queries (DB-12).
macro_rules! candidate_columns {
    () => {
        "id, batch_id, url, state, file_name, size, provider, category_id, priority, route_json, error, error_code, package_id, position, checked_at, cached_at, cached_by, created_at, media_json, request_json, replay_consent_json, torrent_json, listing_json, remote_credential_id, auth_profile_id, auth_profile_pinned, enrichment_json, file_name_declared, mirror_group, mirror_source, mirror_selected, mirror_pinned, mirror_quality, mirror_language, secret_fragment_ref, source_set_json"
    };
}

pub(crate) const CANDIDATE_SELECT: &str =
    concat!("SELECT ", candidate_columns!(), " FROM link_candidates");

pub(crate) const GET_CANDIDATE: &str = concat!(
    "SELECT ",
    candidate_columns!(),
    " FROM link_candidates WHERE id = ?"
);
