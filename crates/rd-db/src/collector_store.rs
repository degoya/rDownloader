use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    BatchId, CandidateId, CollectorBatch, EventEnvelope, EventKind, IngressSource, LinkCandidate,
    LinkCandidateState,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, parse_id};

/// Intake of one submission: links are grouped into packages (JDownloader-style), duplicates
/// are flagged, and – when `auto_check` – every fresh link starts in `checking`.
pub(crate) async fn add_batch(
    connection: &mut SqliteConnection,
    mut intake: NewCollectorBatch,
    secret_fragment_refs: Vec<Option<String>>,
) -> Result<(
    CollectorBatch,
    Vec<rd_core::CollectorPackage>,
    Vec<LinkCandidate>,
    Vec<EventEnvelope>,
)> {
    // Every intake path ends here -- pasted text, a DLC, an NZB import, a hotfolder, a
    // subscription poll, a handed-over torrent -- so this is the one place where the rule that
    // no candidate row carries a fragment can be stated once (RD-109-32).
    //
    // The fragment a declaring provider needs back was already put in the vault by
    // `Database::add_collector_batch`, which is the only caller of this function and the one
    // that holds the vault; what arrives here is a reference per link and an address that is
    // shortened exactly as every other one (RD-110-38).
    for url in &mut intake.urls {
        *url = rd_core::candidate_url(url);
    }
    let batch = CollectorBatch {
        id: BatchId::new(),
        source: intake.source,
        source_label: intake.source_label,
        created_at: Utc::now(),
    };
    let (rules, default_category) = crate::config_store::routing_config(connection).await?;
    let source_value = enum_string(intake.source)?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO collector_batches (id, source, source_label, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(batch.id.to_string())
    .bind(source_value)
    .bind(&batch.source_label)
    .bind(batch.created_at)
    .execute(&mut *transaction)
    .await?;

    let derived_file_names: Vec<Option<String>> = intake
        .urls
        .iter()
        .map(|url| {
            url.path_segments()
                .and_then(Iterator::last)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect();
    let file_names: Vec<Option<String>> = intake
        .urls
        .iter()
        .enumerate()
        .map(|(index, _)| {
            intake
                .file_names
                .get(index)
                .cloned()
                .flatten()
                .or_else(|| derived_file_names[index].clone())
        })
        .collect();
    let hosts: Vec<String> = intake
        .urls
        .iter()
        .map(|url| url.host_str().unwrap_or_default().to_owned())
        .collect();
    // Resolved once: the provider decides both what the candidate is and whether it is a
    // container that takes a package of its own.
    let providers: Vec<String> = intake
        .urls
        .iter()
        .enumerate()
        .map(|(index, url)| {
            intake
                .providers
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or_else(|| provider_for(url))
        })
        .collect();
    let inputs: Vec<rd_collector::GroupInput<'_>> = intake
        .urls
        .iter()
        .enumerate()
        .map(|(index, _)| rd_collector::GroupInput {
            index,
            file_name: file_names[index].as_deref(),
            host: &hosts[index],
            // Only a name the source gave: the address's last segment is no release name —
            // every hit of an indexer shares it.
            standalone: intake.file_names.get(index).is_some_and(Option::is_some)
                && matches!(
                    providers[index].as_str(),
                    rd_core::NZB_PROVIDER | rd_core::TORRENT_PROVIDER
                ),
            package_hint: intake.package_hints.get(index).and_then(Option::as_deref),
        })
        .collect();
    let groups = rd_collector::group_links(&inputs, intake.package_name.as_deref(), "Links");

    let mut packages = Vec::with_capacity(groups.len());
    let mut candidates = Vec::with_capacity(intake.urls.len());
    for group in groups {
        let group_password = group
            .members
            .iter()
            .filter_map(|index| intake.passwords.get(*index))
            .find_map(|password| password.as_deref().filter(|value| !value.is_empty()))
            .or(intake.password.as_deref());
        let mut package_id = None;
        let mut package_category = None;
        for (position, index) in group.members.iter().enumerate() {
            let url = &intake.urls[*index];
            let duplicate = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM link_candidates WHERE url = ? AND state != 'duplicate')",
            )
            .bind(url.as_str())
            .fetch_one(&mut *transaction)
            .await?
                != 0;
            let state = if duplicate {
                LinkCandidateState::Duplicate
            } else if intake.auto_check {
                LinkCandidateState::Checking
            } else {
                LinkCandidateState::Online
            };
            let provider = providers[*index].clone();
            let file_name = file_names[*index].clone();
            let category_id = intake.category_id.or_else(|| {
                rd_collector::select_category(
                    &rules,
                    &rd_collector::CategoryContext {
                        source: intake.source,
                        url,
                        file_name: file_name.as_deref(),
                        mime_type: None,
                    },
                    default_category,
                )
            });
            let priority = intake.priority.unwrap_or_default();
            let package = match package_id {
                Some(id) => id,
                None => {
                    let id = crate::collector_packages::insert(
                        &mut transaction,
                        batch.id,
                        &group.name,
                        // Auto-named means "we guessed this and may guess again". A name the
                        // source stated -- the package name of the request, or the release
                        // title a site rule read off the page -- is not a guess, so the
                        // regroup after the online check leaves it alone (RD-120-17).
                        !group.named_by_source,
                        category_id,
                        priority,
                        group_password,
                    )
                    .await?;
                    package_id = Some(id);
                    package_category = category_id;
                    id
                }
            };
            let candidate = LinkCandidate {
                id: CandidateId::new(),
                batch_id: batch.id,
                url: url.clone(),
                state,
                file_name,
                file_name_declared: intake.file_names.get(*index).is_some_and(Option::is_some),
                size: intake.sizes.get(*index).copied().flatten(),
                provider: Some(provider),
                category_id: package_category,
                priority,
                route: None,
                error: None,
                error_code: None,
                package_id: Some(package),
                position: i64::try_from(position)? + 1,
                checked_at: None,
                cached_at: None,
                cached_by: None,
                created_at: batch.created_at,
                media: None,
                request: intake.requests.get(*index).cloned().flatten(),
                enrichment: Vec::new(),
                // Captures arrive inert: the body is encrypted from the first millisecond,
                // but nothing is replayed until a person consents at enqueue time.
                replay_consent: None,
                torrent: None,
                listing: None,
                remote_credential_id: None,
                // Scope matching decides until somebody picks a profile for this link.
                auth_profile: rd_core::AuthProfileSelection::Auto,
                // Filled by `collector_mirrors::assign` once the batch is complete, and read
                // back from the rows below; a link on its own is a mirror of nothing.
                mirror: None,
                // Set from the column when the batch is re-read below, so the flag a reader
                // sees always comes from what was actually written.
                secret_fragment: false,
            };
            let stored_size = candidate
                .size
                .map(|size| i64::try_from(size.get()))
                .transpose()?;
            let stored_request = candidate
                .request
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let stored_body_ref = intake.body_refs.get(*index).cloned().flatten();
            let stored_fragment_ref = secret_fragment_refs.get(*index).cloned().flatten();
            // What the indexer said about this hit, already through the `attributes.rs` gate
            // (RD-107-02). Absent for every link that has no subscription behind it, which
            // is what "asked without attributes" looks like one layer down.
            let stored_attributes = intake
                .source_attributes
                .get(*index)
                .filter(|map| !map.is_empty())
                .map(serde_json::to_string)
                .transpose()?;
            let mirror = intake.mirror_hints.get(*index).cloned().flatten();
            sqlx::query(
                "INSERT INTO link_candidates (id, batch_id, url, state, file_name, file_name_declared, size, provider, category_id, priority, package_id, position, created_at, request_json, replay_body_ref, source_attributes_json, mirror_declared, mirror_quality, mirror_language, secret_fragment_ref) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(candidate.id.to_string())
            .bind(candidate.batch_id.to_string())
            .bind(candidate.url.as_str())
            .bind(enum_string(candidate.state)?)
            .bind(&candidate.file_name)
            .bind(i64::from(candidate.file_name_declared))
            .bind(stored_size)
            .bind(&candidate.provider)
            .bind(candidate.category_id.map(|id| id.to_string()))
            .bind(i64::from(candidate.priority.as_i32()))
            .bind(package.to_string())
            .bind(candidate.position)
            .bind(batch.created_at)
            .bind(stored_request)
            .bind(stored_body_ref)
            .bind(stored_attributes)
            .bind(
                mirror
                    .as_ref()
                    .map(|hint| hint.group.trim().to_owned())
                    .filter(|group| !group.is_empty()),
            )
            .bind(mirror.as_ref().and_then(|hint| hint.quality.clone()))
            .bind(mirror.as_ref().and_then(|hint| hint.language.clone()))
            .bind(stored_fragment_ref)
            .execute(&mut *transaction)
            .await?;
            candidates.push(candidate);
        }
        if let Some(id) = package_id {
            packages.push(id);
        }
    }
    // After the whole batch is written rather than per link: a mirror is only a mirror
    // relative to the others, so there is nothing to decide until the package is complete.
    crate::collector_mirrors::assign(&mut transaction, &packages).await?;
    let candidates = reread_candidates(&mut transaction, candidates).await?;
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "batch_id": batch.id, "candidate_count": candidates.len() }),
    );
    // A second, narrower event for one import arriving. `collector.changed` also fires for
    // every later edit of a candidate, so it cannot tell an intake apart from an edit — which
    // is exactly what a desktop notification has to distinguish.
    let intake = EventEnvelope::new(
        EventKind::CollectorIntake,
        serde_json::json!({
            "batch_id": batch.id,
            "candidate_count": candidates.len(),
            "package_count": packages.len(),
            "source": batch.source,
        }),
    );
    insert_event(&mut transaction, &event).await?;
    insert_event(&mut transaction, &intake).await?;
    transaction.commit().await?;
    let mut created = Vec::with_capacity(packages.len());
    for id in packages {
        if let Some(package) =
            crate::collector_packages::get_from_connection(connection, id).await?
        {
            created.push(package);
        }
    }
    Ok((batch, created, candidates, vec![event, intake]))
}

/// One LinkGrabber submission.
pub struct NewCollectorBatch {
    pub source: IngressSource,
    pub source_label: Option<String>,
    /// Explicit package name (Click'n'Load `package` field or manual input).
    pub package_name: Option<String>,
    /// Archive password announced with the submission.
    pub password: Option<String>,
    /// Per-link archive passwords, parallel to `urls`.
    ///
    /// A subscription poll can submit several standalone releases at once. Keeping their
    /// passwords beside their links prevents the first release's password from being copied to
    /// every package in the batch. `password` remains the fallback for ordinary batch-wide
    /// intake such as a pasted list or DLC container.
    pub passwords: Vec<Option<String>>,
    /// Explicit category for every created package; `None` applies the normal routing rules.
    pub category_id: Option<rd_core::CategoryId>,
    /// Explicit package priority; `None` uses the normal priority.
    pub priority: Option<rd_core::DownloadPriority>,
    pub urls: Vec<Url>,
    /// Provider override per URL (parallel to `urls`); `None` = derive from the host.
    pub providers: Vec<Option<String>>,
    /// Optional file-name overrides parallel to `urls` (used by parsed local metadata).
    pub file_names: Vec<Option<String>>,
    /// Optional size overrides parallel to `urls`.
    pub sizes: Vec<Option<rd_core::ByteCount>>,
    /// Optional package suggestions parallel to `urls` — the folder a crawler read a link
    /// out of (RD-104-03). Links sharing one become one package; an explicit
    /// `package_name` still overrides all of them.
    pub package_hints: Vec<Option<String>>,
    /// What each link's source said about mirrors, parallel to `urls` (RD-110-18).
    ///
    /// The first of the three sources a mirror group can come from, and the only one that
    /// arrives from outside: a site rule whose page is one release states that its links are
    /// the same file. Stored as it came in and never recomputed, so the regroup that runs
    /// after the online check cannot lose it.
    pub mirror_hints: Vec<Option<rd_core::MirrorHint>>,
    /// Captured request metadata parallel to `urls` (intercepted browser downloads).
    pub requests: Vec<Option<rd_core::CapturedRequest>>,
    /// `vault://` reference of each captured request body, parallel to `urls`.
    ///
    /// Kept out of `requests` on purpose: the reference goes straight into its own column
    /// and never through a serializable struct.
    pub body_refs: Vec<Option<String>>,
    /// Start every fresh link in `checking` so the link check service probes it.
    pub auto_check: bool,
    /// What the source already knows about each link, parallel to `urls` (RD-107-02).
    ///
    /// Filled only by a subscription poll, with the attributes `attributes.rs` retained:
    /// `imdb`, `imdbscore`, `imdbplot`, `coverurl` and whatever else the indexer emitted,
    /// minus everything that gate discards. Reached later by an enricher, so it does not
    /// have to guess a title back out of a file name. An empty map means "nothing declared",
    /// which is every pasted, captured and container link.
    pub source_attributes: Vec<BTreeMap<String, String>>,
}

/// What the indexer declared about the hit behind one candidate (RD-107-02).
///
/// Empty for a link no subscription produced. The caller re-applies the `attributes.rs` gate
/// before anything leaves towards a plugin; this read is not itself that gate.
pub(crate) async fn source_attributes(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<BTreeMap<String, String>> {
    let stored = sqlx::query_scalar::<_, Option<String>>(
        "SELECT source_attributes_json FROM link_candidates WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(&mut *connection)
    .await?
    .flatten();
    Ok(stored
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default())
}

pub(crate) async fn list_batches(pool: &SqlitePool) -> Result<Vec<CollectorBatch>> {
    sqlx::query_as::<_, BatchRow>(
        "SELECT id, source, source_label, created_at FROM collector_batches ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn list_candidates(pool: &SqlitePool) -> Result<Vec<LinkCandidate>> {
    sqlx::query_as::<_, CandidateRow>(&format!(
        "{CANDIDATE_SELECT} WHERE state != 'enqueued' ORDER BY \
         COALESCE((SELECT p.position FROM collector_packages p WHERE p.id = link_candidates.package_id), 0) ASC, \
         position ASC, created_at ASC"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// The `vault://` reference one candidate holds, if any (RD-110-38).
pub(crate) async fn secret_fragment_ref(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<String>> {
    use sqlx::Row;

    let row = sqlx::query("SELECT secret_fragment_ref FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|row| {
        row.try_get::<Option<String>, _>("secret_fragment_ref")
            .ok()
            .flatten()
    }))
}

/// Every reference the rows matching `predicate` hold, read before they are deleted.
///
/// Read first and removed from the vault afterwards, because the reference is *in* the row:
/// once the row is gone there is nothing left to find the secret by, and it would sit in the
/// vault for the life of the installation. The predicate is the same one the delete uses.
async fn secret_fragment_refs_where(
    pool: &SqlitePool,
    predicate: &str,
    bind: Option<String>,
) -> Result<Vec<String>> {
    use sqlx::Row;

    let sql = format!(
        "SELECT secret_fragment_ref FROM link_candidates WHERE secret_fragment_ref IS NOT NULL AND {predicate}"
    );
    let mut query = sqlx::query(&sql);
    if let Some(value) = bind {
        query = query.bind(value);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("secret_fragment_ref")
                .ok()
                .flatten()
        })
        .collect())
}

/// The references `delete_candidates` is about to orphan.
pub(crate) async fn deletable_secret_fragment_refs(pool: &SqlitePool) -> Result<Vec<String>> {
    secret_fragment_refs_where(pool, "state NOT IN ('resolving', 'enqueued')", None).await
}

/// The references deleting one LinkGrabber package is about to orphan.
pub(crate) async fn package_secret_fragment_refs(
    pool: &SqlitePool,
    id: rd_core::CollectorPackageId,
) -> Result<Vec<String>> {
    secret_fragment_refs_where(
        pool,
        "package_id = ? AND state != 'enqueued'",
        Some(id.to_string()),
    )
    .await
}

/// Clears a candidate's reference once a download row owns it.
pub(crate) async fn take_candidate_secret_fragment_ref(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: CandidateId,
) -> Result<()> {
    sqlx::query("UPDATE link_candidates SET secret_fragment_ref = NULL WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(crate) async fn delete_candidate(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<EventEnvelope> {
    let state = sqlx::query_scalar::<_, String>("SELECT state FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("link candidate not found"))?;
    anyhow::ensure!(
        state != "resolving",
        StoreError::busy("link candidate is being processed")
    );
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "candidate_id": id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM link_candidates WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    delete_empty_batches(&mut transaction).await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

pub(crate) async fn delete_candidates(
    connection: &mut SqliteConnection,
) -> Result<(u64, EventEnvelope)> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "all_candidates_removed": true }),
    );
    let mut transaction = connection.begin().await?;
    let result =
        sqlx::query("DELETE FROM link_candidates WHERE state NOT IN ('resolving', 'enqueued')")
            .execute(&mut *transaction)
            .await?;
    delete_empty_batches(&mut transaction).await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected(), event))
}

pub(crate) async fn delete_empty_batches(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    crate::collector_packages::delete_empty_packages(transaction).await?;
    sqlx::query(
        "DELETE FROM collector_batches WHERE NOT EXISTS \
         (SELECT 1 FROM link_candidates WHERE batch_id = collector_batches.id)",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn get_candidate(
    pool: &SqlitePool,
    id: CandidateId,
) -> Result<Option<LinkCandidate>> {
    sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

#[derive(FromRow)]
struct BatchRow {
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
                .and_then(|value| serde_json::from_str(value).ok())
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
        })
    }
}

/// Reads the candidates back after the mirror groups were written onto them.
///
/// The rows were built before the grouping ran — it needs the whole package — so the values
/// in hand are one step behind the database. Returning them as they are would hand the
/// caller, and the event it publishes, a batch in which nothing is a mirror of anything.
async fn reread_candidates(
    connection: &mut SqliteConnection,
    candidates: Vec<LinkCandidate>,
) -> Result<Vec<LinkCandidate>> {
    let mut fresh = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let row = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
            .bind(candidate.id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
        match row {
            Some(row) => fresh.push(row.try_into()?),
            None => fresh.push(candidate),
        }
    }
    Ok(fresh)
}

/// Replaces a candidate's enrichment fields; an empty list clears the column.
///
/// A candidate that has already been handed to the queue has no reader of its own left: the
/// enqueue copies the fields it saw when it claimed the row, and detaches the row when it is
/// done. Fields that arrive after that claim therefore have to be carried onto the queue rows
/// the candidate became, or they reach nothing (RD-108-15).
pub(crate) async fn set_enrichment(
    connection: &mut SqliteConnection,
    id: rd_core::CandidateId,
    fields: &[rd_core::EnrichmentField],
) -> Result<()> {
    let stored = if fields.is_empty() {
        None
    } else {
        Some(serde_json::to_string(fields)?)
    };
    let mut tx = connection.begin().await?;
    sqlx::query("UPDATE link_candidates SET enrichment_json = ? WHERE id = ?")
        .bind(stored)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let handed_over: Option<(String, String)> =
        sqlx::query_as("SELECT state, url FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((state, url)) = handed_over
        && matches!(state.as_str(), "resolving" | "enqueued")
    {
        crate::package_store::carry_enrichment_for_source(&mut tx, &url, fields).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) const CANDIDATE_SELECT: &str = "SELECT id, batch_id, url, state, file_name, size, provider, category_id, priority, route_json, error, error_code, package_id, position, checked_at, cached_at, cached_by, created_at, media_json, request_json, replay_consent_json, torrent_json, listing_json, remote_credential_id, auth_profile_id, auth_profile_pinned, enrichment_json, file_name_declared, mirror_group, mirror_source, mirror_selected, mirror_pinned, mirror_quality, mirror_language, secret_fragment_ref \
     FROM link_candidates";

pub(crate) const GET_CANDIDATE: &str = "SELECT id, batch_id, url, state, file_name, size, provider, category_id, priority, route_json, error, error_code, package_id, position, checked_at, cached_at, cached_by, created_at, media_json, request_json, replay_consent_json, torrent_json, listing_json, remote_credential_id, auth_profile_id, auth_profile_pinned, enrichment_json, file_name_declared, mirror_group, mirror_source, mirror_selected, mirror_pinned, mirror_quality, mirror_language, secret_fragment_ref \
     FROM link_candidates WHERE id = ?";

fn provider_for(url: &Url) -> String {
    rd_provider_registry::provider_for_url(url)
        .map_or_else(|| "direct_http".to_owned(), |spec| spec.slug)
}

pub(crate) fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).context("parse stored enum")
}

pub(crate) async fn insert_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventEnvelope,
) -> Result<()> {
    sqlx::query("INSERT INTO events (id, kind, occurred_at, payload_json) VALUES (?, ?, ?, ?)")
        .bind(event.id.to_string())
        .bind(enum_string(&event.kind)?)
        .bind(event.occurred_at)
        .bind(serde_json::to_string(&event.payload)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
