//! DLC container import: the opt-in call to the decryption service, and the intake that turns
//! a decrypted container into LinkGrabber packages.
//!
//! The format cannot be decrypted locally (see `rd_collector::dlc`), so importing a container
//! means sending its key blob to a third-party service. That is why the feature is off until
//! somebody turns it on: [`decrypt_container`] refuses before it builds a request, never after.
//!
//! Everything here takes plain handles instead of `AppState`, so the HTTP endpoint and the
//! hotfolder watcher share one implementation.

use std::time::Duration;

use rd_collector::{DlcDocument, DlcPackage};
use rd_core::IngressSource;
use rd_db::NewCollectorBatch;

use crate::{
    ApiError,
    dto::SettingsResponse,
    input_checks::{BodyError, read_bounded_body},
    link_check_service::LinkCheckService,
};

/// JDownloader's service, used when the settings name no other one.
pub(crate) const DEFAULT_DLC_ENDPOINT: &str = "http://service.jdownloader.org/dlcrypt/service.php";

/// Timeout of the single request the import makes.
const SERVICE_TIMEOUT: Duration = Duration::from_secs(20);

/// The answer carries one encrypted key; anything larger is not an answer we can use.
const MAX_SERVICE_ANSWER_BYTES: usize = 64 * 1024;

/// Handles the intake needs, so both callers can build it from what they have.
pub struct DlcIntake<'a> {
    pub database: &'a rd_db::Database,
    pub secrets: &'a rd_secrets::SecretStore,
    pub link_check: &'a LinkCheckService,
    pub media: rd_core::MediaSettings,
    pub gallery: rd_core::GallerySettings,
}

/// What the import created, aggregated over every package in the container.
pub struct DlcImportOutcome {
    pub packages: Vec<rd_core::CollectorPackage>,
    pub candidates: Vec<rd_core::LinkCandidate>,
    pub skipped_excluded: u32,
    /// The NZBs an `.rdlinks` file carried (RD-1220-02); empty for every other container.
    pub nzb_imports: Vec<rd_core::NzbImport>,
}

/// Metadata the container itself does not carry.
pub struct DlcImportOptions {
    pub source: IngressSource,
    /// File name of the container, shown as the batch's origin.
    pub source_label: Option<String>,
    /// Package name for containers whose packages are unnamed.
    pub fallback_name: Option<String>,
    /// Archive password from the container's file name (`release{{password}}.dlc`).
    pub fallback_password: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    pub priority: Option<rd_core::DownloadPriority>,
}

/// Unlocks a container, calling the configured decryption service exactly once.
///
/// `source` names the format the service is asked to read: a CCF is unwrapped by the same
/// endpoint, which answers with the DLC inside it.
pub async fn decrypt_container(
    settings: &SettingsResponse,
    content: &[u8],
    source: &str,
) -> Result<DlcDocument, ApiError> {
    if !settings.dlc_service_enabled {
        return Err(ApiError::bad_request(
            "dlc.service_disabled",
            "DLC import is switched off. A DLC can only be decrypted by an online service, so importing one has to be enabled in the settings first",
        ));
    }
    if content.len() > rd_collector::MAX_DLC_BYTES {
        return Err(
            ApiError::bad_request("dlc.too_large", "DLC exceeds the 8 MiB limit")
                .with_param("max_bytes", rd_collector::MAX_DLC_BYTES),
        );
    }
    let container = rd_collector::split_dlc_container(content)
        .map_err(|error| ApiError::bad_request("dlc.file_invalid", format!("{error:#}")))?;
    let endpoint = settings
        .dlc_service_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DLC_ENDPOINT);
    let request = url::Url::parse_with_params(
        endpoint,
        [
            ("srcType", source),
            ("destType", rd_collector::DLCRYPT_DEST_TYPE),
            ("data", container.key_blob()),
        ],
    )
    .map_err(|_| {
        ApiError::bad_request(
            "dlc.endpoint_invalid",
            "The DLC decryption service must be an http or https URL",
        )
    })?;
    let client = reqwest::Client::builder()
        .timeout(SERVICE_TIMEOUT)
        .user_agent(rd_core::user_agent!())
        .build()
        .map_err(anyhow::Error::new)?;
    let response = client
        .get(request)
        .send()
        .await
        .map_err(|error| service_unreachable(&error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(service_unreachable(&format!("HTTP {status}")));
    }
    let answer = bounded_body(response).await?;
    rd_collector::decrypt_dlc(&container, &answer).map_err(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the DLC could not be decrypted");
        ApiError::bad_request(
            "dlc.decrypt_failed",
            "The DLC could not be decrypted with the key the service returned",
        )
        .with_param("detail", truncated(&format!("{error:#}")))
    })
}

/// Reads the answer with a hard cap.
///
/// The endpoint is configurable, so the body is bounded as it arrives rather than trusted to
/// be short: a declared length is checked first, and a chunked answer that keeps coming is
/// abandoned once it passes the cap.
async fn bounded_body(response: reqwest::Response) -> Result<String, ApiError> {
    let body = read_bounded_body(response, MAX_SERVICE_ANSWER_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge => service_unreachable("the answer is too large to be a key"),
            BodyError::Interrupted(error) => service_unreachable(&error.to_string()),
        })?;
    String::from_utf8(body).map_err(|_| service_unreachable("the answer is not text"))
}

/// Turns a decrypted container into one LinkGrabber batch per package it holds.
pub async fn import_document(
    intake: &DlcIntake<'_>,
    document: DlcDocument,
    options: DlcImportOptions,
) -> Result<DlcImportOutcome, ApiError> {
    let total: usize = document
        .packages
        .iter()
        .map(|package| package.files.len())
        .sum();
    if total == 0 {
        return Err(ApiError::bad_request(
            "dlc.no_links",
            "The DLC contains no links",
        ));
    }
    if total > rd_core::MAX_CAPTURE_LINKS {
        return Err(crate::capture_sanitize::links_limit(
            rd_core::MAX_CAPTURE_LINKS,
        ));
    }
    let packages = document
        .packages
        .into_iter()
        .map(|package| {
            let DlcPackage {
                name,
                password,
                files,
                ..
            } = package;
            IncomingPackage {
                name,
                password,
                category_id: options.category_id,
                links: files
                    .into_iter()
                    .map(|file| IncomingLink {
                        url: file.url,
                        file_name: file.file_name,
                        size: file.size,
                        mirror_group: None,
                    })
                    .collect(),
            }
        })
        .collect();
    let outcome = import_packages(intake, packages, &options, None).await?;
    if outcome.packages.is_empty() {
        return Err(all_links_excluded());
    }
    Ok(outcome)
}

/// Takes an `.rdlinks` document into the LinkGrabber (RD-1210-01): one batch per package, the
/// category found again by its name, the mirror groups kept, and every address a proposal held
/// to `reach` (`LinkOrigin::Proposed` of the intake that handed the file over).
///
/// Nothing in the document binds a link to a plugin: each one is assigned to its host again,
/// with an alias host turned into the hoster's own domain, and resolved by the plugin installed
/// now. An explicit `options.category_id` wins over the category the file names.
///
/// The NZBs the file carries become NZB imports like a dropped NZB (RD-1220-02), landing as
/// `nzbs` says; every one is parsed before anything is created, so a broken one refuses the file.
pub async fn import_links(
    intake: &DlcIntake<'_>,
    document: rd_collector::LinksDocument,
    options: DlcImportOptions,
    reach: Option<bool>,
    nzbs: crate::links_nzb::NzbLanding<'_>,
) -> Result<DlcImportOutcome, ApiError> {
    let total = rd_collector::link_count(&document) + rd_collector::nzb_count(&document);
    if total == 0 {
        return Err(ApiError::bad_request(
            "dlc.no_links",
            "The DLC contains no links",
        ));
    }
    if total > rd_collector::MAX_RDLINKS_LINKS {
        return Err(links_limit());
    }
    let categories = intake.database.list_categories().await?;
    let category_of = |named: Option<&str>| {
        options.category_id.or_else(|| {
            let wanted = named?.trim();
            categories
                .iter()
                .find(|category| category.name.eq_ignore_ascii_case(wanted))
                .map(|category| category.id)
        })
    };
    let embedded = crate::links_nzb::parse_embedded(&document, category_of)?;
    let packages = document
        .packages
        .into_iter()
        .filter(|package| !package.links.is_empty())
        .map(|package| IncomingPackage {
            category_id: category_of(package.category.as_deref()),
            name: package.name,
            password: package.password,
            links: package
                .links
                .into_iter()
                .map(|link| IncomingLink {
                    url: rd_collector::canonical_url(link.url),
                    file_name: link.file_name,
                    size: link.size,
                    mirror_group: link.mirror_group,
                })
                .collect(),
        })
        .collect();
    let mut outcome = import_packages(intake, packages, &options, reach).await?;
    outcome.nzb_imports =
        crate::links_nzb::store_embedded(intake.database, embedded, &options, nzbs).await?;
    if outcome.packages.is_empty() && outcome.nzb_imports.is_empty() {
        return Err(all_links_excluded());
    }
    Ok(outcome)
}

/// `400` for a link file or an export over [`rd_collector::MAX_RDLINKS_LINKS`].
#[must_use]
pub fn links_limit() -> ApiError {
    ApiError::bad_request(
        "rdlinks.links_limit",
        format!(
            "A link file holds at most {} links",
            rd_collector::MAX_RDLINKS_LINKS
        ),
    )
    .with_param("max", rd_collector::MAX_RDLINKS_LINKS)
}

/// One package on its way into the LinkGrabber, whatever file it came from.
struct IncomingPackage {
    name: Option<String>,
    password: Option<String>,
    category_id: Option<rd_core::CategoryId>,
    links: Vec<IncomingLink>,
}

struct IncomingLink {
    url: url::Url,
    file_name: Option<String>,
    size: Option<u64>,
    mirror_group: Option<String>,
}

async fn import_packages(
    intake: &DlcIntake<'_>,
    packages: Vec<IncomingPackage>,
    options: &DlcImportOptions,
    reach: Option<bool>,
) -> Result<DlcImportOutcome, ApiError> {
    let excluded = crate::collector_exclusions::blocklist(intake.database).await?;
    let mut outcome = DlcImportOutcome {
        packages: Vec::new(),
        candidates: Vec::new(),
        skipped_excluded: 0,
        nzb_imports: Vec::new(),
    };
    for package in packages {
        let before = package.links.len();
        let files: Vec<IncomingLink> = package
            .links
            .into_iter()
            .filter(|file| {
                !file
                    .url
                    .host_str()
                    .is_some_and(|host| crate::collector_exclusions::is_excluded(&excluded, host))
            })
            .collect();
        outcome.skipped_excluded += u32::try_from(before - files.len()).unwrap_or(u32::MAX);
        if files.is_empty() {
            continue;
        }
        let mut urls: Vec<url::Url> = files.iter().map(|file| file.url.clone()).collect();
        // A container may carry `ftp://user:pw@host/…` just like a pasted link does; the
        // password is vaulted here so it never reaches a candidate row.
        crate::collector_intake::adopt_remote_credentials(
            intake.database,
            intake.secrets,
            &mut urls,
        )
        .await?;
        let providers =
            crate::collector_intake::providers_for(&urls, &intake.media, &intake.gallery);
        let file_names = files
            .iter()
            .map(|file| {
                file.file_name
                    .as_deref()
                    .map(rd_files::sanitize_file_name)
                    .filter(|name| !name.is_empty())
            })
            .collect();
        let sizes = files
            .iter()
            .map(|file| {
                file.size
                    .and_then(|size| rd_core::ByteCount::new(size).ok())
            })
            .collect();
        let mirror_hints = if files.iter().any(|file| file.mirror_group.is_some()) {
            files
                .iter()
                .map(|file| {
                    file.mirror_group.clone().map(|group| rd_core::MirrorHint {
                        group,
                        quality: None,
                        language: None,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let (batch, packages, candidates) = intake
            .database
            .add_collector_batch(NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints,
                source: options.source,
                source_label: options.source_label.clone(),
                package_name: package_name(
                    package.name.as_deref(),
                    options.fallback_name.as_deref(),
                ),
                password: package
                    .password
                    .or_else(|| options.fallback_password.clone()),
                passwords: Vec::new(),
                category_id: package.category_id,
                priority: options.priority,
                providers,
                urls,
                file_names,
                sizes,
                requests: Vec::new(),
                body_refs: Vec::new(),
                auto_check: true,
                source_attributes: Vec::new(),
            })
            .await?;
        // Before the check starts: the online check of an address somebody else chose keeps to
        // the same address rule as the transfer (RD-150-03).
        if let Some(local_network) = reach {
            let ids = candidates.iter().map(|candidate| candidate.id).collect();
            intake
                .database
                .set_candidates_remote_reach(ids, local_network)
                .await?;
        }
        intake.link_check.check_batch(batch.id).await;
        outcome.packages.extend(packages);
        outcome.candidates.extend(candidates);
    }
    Ok(outcome)
}

fn all_links_excluded() -> ApiError {
    ApiError::bad_request(
        "collector.all_links_excluded",
        "All links were skipped by the domain blocklist",
    )
}

/// The container's own package name wins; without one the file name keeps the packages of a
/// multi-package container apart, and grouping decides the rest.
fn package_name(declared: Option<&str>, fallback: Option<&str>) -> Option<String> {
    declared
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or(fallback)
        .map(|name| name.chars().take(200).collect())
}

fn service_unreachable(detail: &str) -> ApiError {
    // Logged as well as returned: the import runs from the hotfolder too, where nobody is
    // watching for a toast, and a third-party outage is otherwise indistinguishable from
    // "nothing happened".
    tracing::warn!(detail = %truncated(detail), "the DLC decryption service did not answer");
    ApiError::bad_gateway(
        "dlc.service_unreachable",
        "The DLC decryption service did not answer",
    )
    .with_param("detail", truncated(detail))
}

/// Keeps a foreign service's wording out of an unbounded error field.
fn truncated(detail: &str) -> String {
    detail.chars().take(200).collect()
}
