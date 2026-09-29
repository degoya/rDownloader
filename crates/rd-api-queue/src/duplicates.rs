//! Source and content duplicates, explained apart (RD-150-01).
//!
//! A **source** duplicate is the same thing asked for twice — the same address, magnet, NZB
//! file or hoster file — and is known before a byte is fetched. A **content** duplicate is the
//! same bytes, and is known from the content index once a file is hashed, or before the
//! transfer when the source stated a SHA-256. The answer keeps the two in separate lists
//! because they mean different things: "this is already queued" against "these bytes are
//! already on disk".

use std::{collections::HashMap, path::Path};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rd_core::{
    ChecksumAlgorithm, DownloadFile, DownloadId, DownloadKind, DownloadState, NzbImportId,
    PackageId, SourceIdentity,
};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{AppState, error::ApiError, error_codes::parse_id};

/// Where a source duplicate was found.
#[derive(Clone, Copy, Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateLocation {
    /// A download in the queue, running or finished.
    Queue,
    /// A link waiting in the LinkGrabber.
    Linkgrabber,
}

/// The same source, somewhere else.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SourceDuplicate {
    pub location: DuplicateLocation,
    pub download_id: Option<DownloadId>,
    /// LinkGrabber candidate id, for a link that is not queued yet.
    pub candidate_id: Option<String>,
    pub package_name: Option<String>,
    pub file_name: Option<String>,
    pub state: Option<DownloadState>,
}

/// What a content duplicate is known from.
#[derive(Clone, Copy, Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentBasis {
    /// The download's own finished file was hashed.
    VerifiedHash,
    /// The download has not finished; its source stated this SHA-256.
    StatedChecksum,
}

/// The same bytes, in another finished file.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ContentDuplicate {
    pub download_id: DownloadId,
    pub package_id: PackageId,
    pub package_name: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    /// The file is not where the index says, as of the last check.
    pub missing: bool,
    /// Whether it lies on the same file system as this download's file — the precondition of a
    /// hard link; `None` where the platform cannot say or this download has no file yet.
    pub same_file_system: Option<bool>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DuplicateReport {
    pub download_id: DownloadId,
    pub identity: SourceIdentity,
    pub source: Vec<SourceDuplicate>,
    /// `None` when there is no digest to compare with yet.
    pub content_basis: Option<ContentBasis>,
    /// SHA-256 the content comparison used.
    pub digest: Option<String>,
    pub content: Vec<ContentDuplicate>,
}

#[derive(Deserialize, ToSchema)]
pub struct DuplicateLookupRequest {
    /// Addresses to look up, as the LinkGrabber shows them (at most 500).
    pub urls: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DuplicateLookupEntry {
    pub url: String,
    pub identity: SourceIdentity,
    /// Queue downloads of the same source. The LinkGrabber marks duplicates among its own links
    /// already; this is the part it cannot see.
    pub queue: Vec<SourceDuplicate>,
}

const MAX_LOOKUP_URLS: usize = 500;

/// The identity of an address: a magnet by its info-hash, a hoster link by provider and
/// canonical address, anything else normalised.
#[must_use]
pub(crate) fn identity_of_url(url: &Url) -> SourceIdentity {
    if url.scheme() == "magnet" {
        return SourceIdentity::of_url(url);
    }
    let canonical = rd_collector::canonical_url(url.clone());
    match rd_provider_registry::provider_for_url(&canonical) {
        Some(provider) => SourceIdentity::provider(&provider.slug, &canonical),
        None => SourceIdentity::of_url(&canonical),
    }
}

/// The identity of a queued file. A Usenet file is its NZB's document hash and its own name;
/// its `nzb://` address only names the import row, which a second import of the same NZB
/// would not share.
fn identity_of_download(
    file: &DownloadFile,
    packages: &HashMap<PackageId, rd_core::DownloadPackage>,
    nzb_hashes: &HashMap<NzbImportId, String>,
) -> SourceIdentity {
    if file.kind == DownloadKind::Usenet
        && let Some(hash) = packages
            .get(&file.package_id)
            .and_then(|package| package.nzb_import_id)
            .and_then(|import| nzb_hashes.get(&import))
    {
        return SourceIdentity::nzb(hash, &file.file_name);
    }
    identity_of_url(&file.source)
}

struct Queue {
    downloads: Vec<DownloadFile>,
    packages: HashMap<PackageId, rd_core::DownloadPackage>,
    nzb_hashes: HashMap<NzbImportId, String>,
}

impl Queue {
    async fn load(state: &AppState) -> Result<Self, ApiError> {
        Ok(Self {
            downloads: state.database.list_downloads().await?,
            packages: state
                .database
                .list_packages()
                .await?
                .into_iter()
                .map(|package| (package.id, package))
                .collect(),
            nzb_hashes: state
                .database
                .list_nzb_imports()
                .await?
                .into_iter()
                .map(|import| (import.id, import.sha256))
                .collect(),
        })
    }

    fn identity(&self, file: &DownloadFile) -> SourceIdentity {
        identity_of_download(file, &self.packages, &self.nzb_hashes)
    }

    fn package_name(&self, id: PackageId) -> Option<String> {
        self.packages.get(&id).map(|package| package.name.clone())
    }

    /// Queue downloads with this identity, `except` one.
    fn matching(
        &self,
        identity: &SourceIdentity,
        except: Option<DownloadId>,
    ) -> Vec<SourceDuplicate> {
        self.downloads
            .iter()
            .filter(|other| Some(other.id) != except && self.identity(other) == *identity)
            .map(|other| SourceDuplicate {
                location: DuplicateLocation::Queue,
                download_id: Some(other.id),
                candidate_id: None,
                package_name: self.package_name(other.package_id),
                file_name: Some(other.file_name.clone()),
                state: Some(other.state),
            })
            .collect()
    }
}

/// Source and content duplicates of one download.
#[utoipa::path(get, path = "/api/v1/downloads/{id}/duplicates", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = DuplicateReport), (status = 404)))]
pub async fn download_duplicates(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<DuplicateReport>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    let queue = Queue::load(&state).await?;
    let file = queue
        .downloads
        .iter()
        .find(|file| file.id == id)
        .cloned()
        .ok_or_else(crate::error_codes::download_not_found)?;
    let identity = queue.identity(&file);
    let mut source = queue.matching(&identity, Some(id));
    for candidate in state.database.list_candidates().await? {
        if identity_of_url(&candidate.url) == identity {
            source.push(SourceDuplicate {
                location: DuplicateLocation::Linkgrabber,
                download_id: None,
                candidate_id: Some(candidate.id.to_string()),
                package_name: None,
                file_name: candidate.file_name.clone(),
                state: None,
            });
        }
    }

    let own = state.database.content_index_entry(id).await?;
    let stated = file
        .expected_checksum
        .as_ref()
        .filter(|checksum| checksum.algorithm == ChecksumAlgorithm::Sha256)
        .map(|checksum| checksum.value.to_ascii_lowercase());
    let (content_basis, digest) = match (&own, stated) {
        (Some(entry), _) => (Some(ContentBasis::VerifiedHash), Some(entry.digest.clone())),
        (None, Some(stated)) => (Some(ContentBasis::StatedChecksum), Some(stated)),
        (None, None) => (None, None),
    };
    let mut content = Vec::new();
    if let Some(digest) = &digest {
        let own_path = own.as_ref().map(|entry| entry.path.clone());
        for entry in state
            .database
            .content_index_matches("sha256", digest)
            .await?
        {
            if entry.download_id == id {
                continue;
            }
            let Some(other) = queue
                .downloads
                .iter()
                .find(|other| other.id == entry.download_id)
            else {
                continue;
            };
            let present = tokio::fs::metadata(&entry.path).await.is_ok();
            content.push(ContentDuplicate {
                download_id: entry.download_id,
                package_id: other.package_id,
                package_name: queue.package_name(other.package_id).unwrap_or_default(),
                file_name: other.file_name.clone(),
                same_file_system: own_path.as_deref().and_then(|own_path| {
                    rd_files::same_file_system(Path::new(own_path), Path::new(&entry.path))
                }),
                path: entry.path,
                size_bytes: entry.size_bytes,
                missing: !present,
            });
        }
    }
    Ok(Json(DuplicateReport {
        download_id: id,
        identity,
        source,
        content_basis,
        digest,
        content,
    }))
}

/// The queue's downloads of each address, for the LinkGrabber's "already queued" mark.
#[utoipa::path(post, path = "/api/v1/duplicates/lookup", tag = "collector", request_body = DuplicateLookupRequest, responses((status = 200, body = [DuplicateLookupEntry]), (status = 400)))]
pub async fn lookup_duplicates(
    State(state): State<AppState>,
    Json(request): Json<DuplicateLookupRequest>,
) -> Result<Json<Vec<DuplicateLookupEntry>>, ApiError> {
    if request.urls.len() > MAX_LOOKUP_URLS {
        return Err(crate::error_codes::bulk_range(MAX_LOOKUP_URLS));
    }
    let queue = Queue::load(&state).await?;
    let identities = queue
        .downloads
        .iter()
        .map(|file| (queue.identity(file), file))
        .collect::<Vec<_>>();
    let mut entries = Vec::with_capacity(request.urls.len());
    for raw in request.urls {
        let url = Url::parse(raw.trim()).map_err(|_| {
            ApiError::bad_request(
                "duplicates.url_invalid",
                "Every entry must be an absolute URL",
            )
        })?;
        let identity = identity_of_url(&url);
        let queued = identities
            .iter()
            .filter(|(other, _)| *other == identity)
            .map(|(_, file)| SourceDuplicate {
                location: DuplicateLocation::Queue,
                download_id: Some(file.id),
                candidate_id: None,
                package_name: queue.package_name(file.package_id),
                file_name: Some(file.file_name.clone()),
                state: Some(file.state),
            })
            .collect();
        entries.push(DuplicateLookupEntry {
            url: raw,
            identity,
            queue: queued,
        });
    }
    Ok(Json(entries))
}

#[cfg(test)]
mod tests {
    use super::identity_of_url;

    /// The collector's canonical address is applied first, so a short link and the page it
    /// stands for are one source. (Hoster aliases take the same path, but come from installed
    /// plugins, which a unit test does not load.)
    #[test]
    fn a_short_link_and_its_canonical_address_are_one_source() {
        let short = identity_of_url(&"https://youtu.be/abc123".parse().expect("url"));
        let canonical = identity_of_url(
            &"http://www.youtube.com/watch?v=abc123#t"
                .parse()
                .expect("url"),
        );
        assert_eq!(short, canonical);
    }

    #[test]
    fn a_magnet_is_identified_by_its_hash() {
        let identity = identity_of_url(
            &"magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x"
                .parse()
                .expect("url"),
        );
        assert_eq!(identity.kind, rd_core::SourceIdentityKind::Magnet);
    }
}
