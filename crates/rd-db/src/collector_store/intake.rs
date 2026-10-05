//! Intake of one LinkGrabber submission (`add_batch`), in named steps.

use anyhow::Result;
use chrono::Utc;
use rd_core::{
    BatchId, CandidateId, CollectorBatch, CollectorPackageId, EventEnvelope, EventKind,
    LinkCandidate, LinkCandidateState,
};
use sqlx::{Connection, Sqlite, SqliteConnection, Transaction};
use url::Url;

use super::{BatchPasswords, NewCollectorBatch, insert_event, provider_for, reread_candidates};
use crate::enum_string;

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
    BatchPasswords,
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
        source_label: intake.source_label.take(),
        created_at: Utc::now(),
    };
    let (rules, default_category) = crate::config_store::routing_config(connection).await?;
    // Prepared once for the whole batch: every link is routed against the same rules.
    let routing = rd_collector::CategoryRules::new(&rules);
    let source_value = enum_string(intake.source)?;
    let mut transaction = connection.begin().await?;
    insert_batch_row(&mut transaction, &batch, source_value).await?;

    let links = LinkFacts::of(&intake);
    let groups = links.groups(&intake);
    let write = BatchWrite {
        intake: &intake,
        links: &links,
        routing: &routing,
        default_category,
        batch: &batch,
        secret_fragment_refs: &secret_fragment_refs,
    };
    let mut packages = Vec::with_capacity(groups.len());
    let mut passwords = BatchPasswords::new();
    let mut candidates = Vec::with_capacity(intake.urls.len());
    for group in groups {
        let package = write
            .write_group(&mut transaction, &group, &mut passwords, &mut candidates)
            .await?;
        if let Some(id) = package {
            packages.push(id);
        }
    }
    // After the whole batch is written rather than per link: a mirror is only a mirror
    // relative to the others, so there is nothing to decide until the package is complete.
    crate::collector_mirrors::assign(&mut transaction, &packages).await?;
    let candidates = reread_candidates(&mut transaction, candidates).await?;
    let events = intake_events(&batch, candidates.len(), packages.len());
    for event in &events {
        insert_event(&mut transaction, event).await?;
    }
    transaction.commit().await?;
    let created = created_packages(connection, packages).await?;
    Ok((batch, created, candidates, passwords, events))
}

async fn insert_batch_row(
    transaction: &mut SqliteConnection,
    batch: &CollectorBatch,
    source_value: String,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO collector_batches (id, source, source_label, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(batch.id.to_string())
    .bind(source_value)
    .bind(&batch.source_label)
    .bind(batch.created_at)
    .execute(&mut *transaction)
    .await?;
    Ok(())
}

/// What each link of the batch is before it is grouped: file name, host and provider, parallel
/// to `urls`.
struct LinkFacts {
    file_names: Vec<Option<String>>,
    hosts: Vec<String>,
    providers: Vec<String>,
}

impl LinkFacts {
    fn of(intake: &NewCollectorBatch) -> Self {
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
        Self {
            file_names,
            hosts,
            providers,
        }
    }

    fn groups(&self, intake: &NewCollectorBatch) -> Vec<rd_collector::Group> {
        let inputs: Vec<rd_collector::GroupInput<'_>> = intake
            .urls
            .iter()
            .enumerate()
            .map(|(index, _)| rd_collector::GroupInput {
                index,
                file_name: self.file_names[index].as_deref(),
                host: &self.hosts[index],
                // Only a name the source gave: the address's last segment is no release name —
                // every hit of an indexer shares it.
                standalone: intake.file_names.get(index).is_some_and(Option::is_some)
                    && matches!(
                        self.providers[index].as_str(),
                        rd_core::NZB_PROVIDER | rd_core::TORRENT_PROVIDER
                    ),
                package_hint: intake.package_hints.get(index).and_then(Option::as_deref),
            })
            .collect();
        rd_collector::group_links(&inputs, intake.package_name.as_deref(), "Links")
    }
}

/// Everything the links of one batch are written against.
struct BatchWrite<'a> {
    intake: &'a NewCollectorBatch,
    links: &'a LinkFacts,
    routing: &'a rd_collector::CategoryRules<'a>,
    default_category: Option<rd_core::CategoryId>,
    batch: &'a CollectorBatch,
    secret_fragment_refs: &'a [Option<String>],
}

impl BatchWrite<'_> {
    /// Writes one group's links; its package is created with the first of them and answered.
    async fn write_group(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        group: &rd_collector::Group,
        passwords: &mut BatchPasswords,
        candidates: &mut Vec<LinkCandidate>,
    ) -> Result<Option<CollectorPackageId>> {
        let group_password = group
            .members
            .iter()
            .filter_map(|index| self.intake.passwords.get(*index))
            .find_map(|password| password.as_deref().filter(|value| !value.is_empty()))
            .or(self.intake.password.as_deref());
        let mut package_id = None;
        let mut package_category = None;
        for (position, index) in group.members.iter().enumerate() {
            let url = &self.intake.urls[*index];
            let state = self.candidate_state(transaction, url).await?;
            let category_id = self.intake.category_id.or_else(|| {
                self.routing.select(
                    &rd_collector::CategoryContext {
                        source: self.intake.source,
                        url,
                        file_name: self.links.file_names[*index].as_deref(),
                        mime_type: None,
                    },
                    self.default_category,
                )
            });
            let priority = self.intake.priority.unwrap_or_default();
            let package = match package_id {
                Some(id) => id,
                None => {
                    let id = crate::collector_packages::insert(
                        &mut *transaction,
                        self.batch.id,
                        &group.name,
                        // Auto-named means "we guessed this and may guess again". A name the
                        // source stated -- the package name of the request, or the release
                        // title a site rule read off the page -- is not a guess, so the
                        // regroup after the online check leaves it alone (RD-120-17).
                        !group.named_by_source,
                        category_id,
                        priority,
                    )
                    .await?;
                    // Not a column: `Database::add_collector_batch` puts it in the vault once
                    // the batch is in (RD-190-04).
                    if let Some(password) = group_password {
                        passwords.push((id, password.to_owned()));
                    }
                    package_id = Some(id);
                    package_category = category_id;
                    id
                }
            };
            let candidate =
                self.candidate(*index, position, state, package, package_category, priority)?;
            self.insert_candidate(transaction, &candidate, *index, package)
                .await?;
            candidates.push(candidate);
        }
        Ok(package_id)
    }

    /// A duplicate is an address that is still here: in the LinkGrabber, or in the download
    /// list, finished or not. An `enqueued` candidate row only records that the address was
    /// handed over once; if its download was deleted since, the address is new again (a DLC
    /// imported, queued, deleted and imported again was reported as a duplicate).
    async fn candidate_state(
        &self,
        transaction: &mut SqliteConnection,
        url: &Url,
    ) -> Result<LinkCandidateState> {
        let duplicate = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM link_candidates \
                 WHERE url = ? AND state NOT IN ('duplicate', 'enqueued')) \
             OR EXISTS(SELECT 1 FROM downloads WHERE source_url = ?)",
        )
        .bind(url.as_str())
        .bind(url.as_str())
        .fetch_one(&mut *transaction)
        .await?
            != 0;
        Ok(if duplicate {
            LinkCandidateState::Duplicate
        } else if self.intake.auto_check {
            LinkCandidateState::Checking
        } else {
            LinkCandidateState::Online
        })
    }

    fn candidate(
        &self,
        index: usize,
        position: usize,
        state: LinkCandidateState,
        package: CollectorPackageId,
        package_category: Option<rd_core::CategoryId>,
        priority: rd_core::DownloadPriority,
    ) -> Result<LinkCandidate> {
        Ok(LinkCandidate {
            id: CandidateId::new(),
            batch_id: self.batch.id,
            url: self.intake.urls[index].clone(),
            state,
            file_name: self.links.file_names[index].clone(),
            file_name_declared: self
                .intake
                .file_names
                .get(index)
                .is_some_and(Option::is_some),
            size: self.intake.sizes.get(index).copied().flatten(),
            provider: Some(self.links.providers[index].clone()),
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
            created_at: self.batch.created_at,
            media: None,
            request: self.intake.requests.get(index).cloned().flatten(),
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
            sources: Vec::new(),
        })
    }

    async fn insert_candidate(
        &self,
        transaction: &mut SqliteConnection,
        candidate: &LinkCandidate,
        index: usize,
        package: CollectorPackageId,
    ) -> Result<()> {
        let stored_size = candidate
            .size
            .map(|size| i64::try_from(size.get()))
            .transpose()?;
        let stored_request = candidate
            .request
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let stored_body_ref = self.intake.body_refs.get(index).cloned().flatten();
        let stored_fragment_ref = self.secret_fragment_refs.get(index).cloned().flatten();
        // What the indexer said about this hit, already through the `attributes.rs` gate
        // (RD-107-02). Absent for every link that has no subscription behind it, which
        // is what "asked without attributes" looks like one layer down.
        let stored_attributes = self
            .intake
            .source_attributes
            .get(index)
            .filter(|map| !map.is_empty())
            .map(serde_json::to_string)
            .transpose()?;
        let mirror = self.intake.mirror_hints.get(index).cloned().flatten();
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
        .bind(self.batch.created_at)
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
        Ok(())
    }
}

/// `collector.changed` and the narrower `collector.intake` of one batch, in that order.
fn intake_events(
    batch: &CollectorBatch,
    candidate_count: usize,
    package_count: usize,
) -> Vec<EventEnvelope> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "batch_id": batch.id, "candidate_count": candidate_count }),
    );
    // A second, narrower event for one import arriving. `collector.changed` also fires for
    // every later edit of a candidate, so it cannot tell an intake apart from an edit — which
    // is exactly what a desktop notification has to distinguish.
    let intake = EventEnvelope::new(
        EventKind::CollectorIntake,
        serde_json::json!({
            "batch_id": batch.id,
            "candidate_count": candidate_count,
            "package_count": package_count,
            "source": batch.source,
        }),
    );
    vec![event, intake]
}

async fn created_packages(
    connection: &mut SqliteConnection,
    packages: Vec<CollectorPackageId>,
) -> Result<Vec<rd_core::CollectorPackage>> {
    let mut created = Vec::with_capacity(packages.len());
    for id in packages {
        if let Some(package) =
            crate::collector_packages::get_from_connection(connection, id).await?
        {
            created.push(package);
        }
    }
    Ok(created)
}
