//! The LinkGrabber intake: links in, a checked batch out.

use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rd_api_core::input_checks::optional_text;
use rd_collector::extract_urls;
use rd_db::NewCollectorBatch;

use super::crawl::expand_crawled_links;
use super::links::{CapturedLink, LinkOrigin};
use crate::{
    ApiError, AppState,
    collector_intake::{adopt_remote_credentials, is_service_disabled, providers_for},
    dto::{CaptureLinkRequest, CollectorIntakeRequest, CollectorIntakeResponse},
};

pub async fn collector_intake_inner(
    state: &AppState,
    request: CollectorIntakeRequest,
) -> Result<CollectorIntakeResponse, ApiError> {
    // Structured links carry per-link metadata; free text keeps the legacy paste path.
    let mut links = structured_links(state, request.links).await?;
    let text = request.text.as_deref().unwrap_or_default();
    let source_sets = add_text_links(state, text, &mut links).await;
    if links.is_empty() {
        return Err(ApiError::bad_request(
            "collector.no_links_found",
            "No HTTP(S) links found",
        ));
    }
    // An address that points at many files becomes those files here, before anything else
    // has looked at it (RD-104-03). This is the step `premiumize.multi_file_source` used to
    // promise and nothing performed. What comes back is a proposal like any other: the
    // blocklist below, the disabled-service refusal, the review and the routing rules all
    // apply to it exactly as they do to a pasted link.
    // Whether the person handed this intake over themselves (RD-150-03). A page a stranger
    // wrote is crawled without reaching into this machine or the person's network.
    let own_hand = crate::collector_source_sets::from_own_hand(request.source);
    let crawl_guard = (!own_hand).then(|| state.scheduler.remote_address_policy(false));
    let crawled = expand_crawled_links(state, links, crawl_guard.as_ref()).await?;
    let links = crawled.links;
    let excluded = crate::collector_exclusions::blocklist(&state.database).await?;
    let (links, skipped) = drop_excluded(state, links, &excluded).await?;
    let settings = crate::settings_store::read_settings(state).await?;
    let media_settings = state.media_settings.read().await.clone();
    let gallery_settings = state.gallery_settings.read().await.clone();
    let (links, skipped_disabled) =
        drop_disabled(state, links, &settings, &media_settings, &gallery_settings).await?;
    let guarded_flags: Vec<bool> = links
        .iter()
        .map(|link| link.origin.guarded(own_hand))
        .collect();
    let (mut urls, file_names, sizes, package_hints, mirror_hints, requests, body_refs) =
        CapturedLink::split(links);
    adopt_remote_credentials(&state.database, &state.secrets, &mut urls).await?;
    let guarded_urls: Vec<url::Url> = urls
        .iter()
        .zip(&guarded_flags)
        .filter(|(_, guarded)| **guarded)
        .map(|(url, _)| url.clone())
        .collect();
    let providers = providers_for(&urls, &media_settings, &gallery_settings);
    let (batch, packages, candidates) = state
        .database
        .add_collector_batch(NewCollectorBatch {
            package_hints,
            mirror_hints,
            source: request.source,
            source_label: request.source_label,
            package_name: optional_text(request.package_name),
            password: optional_text(request.password),
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers,
            urls,
            file_names,
            sizes,
            requests,
            body_refs,
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await?;
    // Before the check starts: an auto-queued package must not reach the queue without the
    // sources its links came with.
    crate::collector_source_sets::attach(state, &candidates, source_sets, &excluded, own_hand)
        .await?;
    // Also before the check: the online check of a link a document or a page proposed keeps
    // to the same address rule as the transfer (RD-150-03).
    let guarded: Vec<rd_core::CandidateId> = candidates
        .iter()
        .filter(|candidate| guarded_urls.contains(&candidate.url))
        .map(|candidate| candidate.id)
        .collect();
    if !guarded.is_empty() {
        state
            .database
            .set_candidates_remote_reach(guarded, own_hand)
            .await?;
    }
    state.link_check.check_batch(batch.id).await;
    Ok(CollectorIntakeResponse {
        batch,
        packages,
        candidates,
        skipped_excluded: u32::try_from(skipped).unwrap_or(u32::MAX),
        skipped_disabled,
        crawled_found: crawled.found,
        crawled_dropped: crawled.dropped,
    })
}

/// The links a client sent one by one, each with the request it captured alongside.
async fn structured_links(
    state: &AppState,
    requested: Vec<CaptureLinkRequest>,
) -> Result<Vec<CapturedLink>, ApiError> {
    if requested.len() > rd_core::MAX_CAPTURE_LINKS {
        return Err(crate::capture_sanitize::links_limit(
            rd_core::MAX_CAPTURE_LINKS,
        ));
    }
    let mut links: Vec<CapturedLink> = Vec::new();
    for (index, link) in requested.into_iter().enumerate() {
        // The refusal names the position, never the address: a link that does not parse is the
        // likely one to carry a share password in its fragment (RD-109-39).
        let url = url::Url::parse(link.url.trim())
            .map_err(|_| crate::capture_sanitize::link_url_invalid(index + 1))?;
        let sanitized = link
            .request
            .map(|request| crate::capture_sanitize::sanitize(&url, request))
            .transpose();
        let sanitized = match sanitized {
            Ok(sanitized) => sanitized,
            Err(error) => {
                // Earlier links in the same batch may already have vaulted a body.
                CapturedLink::discard_bodies(state, &links).await;
                return Err(error);
            }
        };
        let (request, body_bytes) = match sanitized {
            Some((request, bytes)) => (Some(request), bytes),
            None => (None, None),
        };
        // Encrypted before it is persisted and before anyone can read it back: the body is
        // inert until a person consents, but it is never at rest in plaintext.
        let body_ref = match body_bytes {
            Some(bytes) => Some(
                state
                    .secrets
                    .put_string(BASE64.encode(&bytes))
                    .await
                    .context("store captured request body")?,
            ),
            None => None,
        };
        links.push(CapturedLink {
            url,
            file_name: optional_text(link.file_name),
            size: None,
            package_hint: None,
            mirror: None,
            request,
            body_ref,
            origin: LinkOrigin::Person,
        });
    }
    Ok(links)
}

/// Adds the links found in free text, then the ones installed intake parsers propose from it,
/// and returns every source set a parser stated for a file it proposed.
async fn add_text_links(
    state: &AppState,
    text: &str,
    links: &mut Vec<CapturedLink>,
) -> Vec<(url::Url, rd_core::SourceSet)> {
    links.extend(extract_urls(text).into_iter().map(CapturedLink::plain));
    // Installed parsers see the same text and may propose links the native scanner does not
    // recognise. They propose only: everything below — the blocklist, the review, the
    // routing rules — applies to their candidates exactly as it does to a pasted link.
    // Every source a parser states for a file it proposed (RD-150-03). Kept aside until the
    // candidates exist, then attached to the one proposed under the same address.
    let mut source_sets = Vec::new();
    if !state.intake_parsers.is_empty() && !text.trim().is_empty() {
        for candidate in state.intake_parsers.parse(text).await {
            if links.iter().any(|link| link.url == candidate.url) {
                continue;
            }
            links.push(CapturedLink::proposed(candidate.url, candidate.file_name));
        }
        source_sets = state.intake_parsers.source_sets(text).await;
    }
    source_sets
}

/// Drops every link whose host is on the domain blocklist, and refuses an intake it empties.
async fn drop_excluded(
    state: &AppState,
    links: Vec<CapturedLink>,
    excluded: &[String],
) -> Result<(Vec<CapturedLink>, usize), ApiError> {
    let (links, skipped): (Vec<_>, Vec<_>) = links.into_iter().partition(|link| {
        !link
            .url
            .host_str()
            .is_some_and(|host| crate::collector_exclusions::is_excluded(excluded, host))
    });
    // A link the blocklist drops never reaches the collector, so its vaulted body has no
    // owner and must not linger in the secret store.
    CapturedLink::discard_bodies(state, &skipped).await;
    if links.is_empty() {
        return Err(ApiError::bad_request(
            "collector.all_links_excluded",
            "All links were skipped by the domain blocklist",
        ));
    }
    Ok((links, skipped.len()))
}

/// Refuses every link whose transfer service is switched off, counts them, and refuses an
/// intake that leaves nothing.
async fn drop_disabled(
    state: &AppState,
    mut links: Vec<CapturedLink>,
    settings: &crate::dto::SettingsResponse,
    media_settings: &rd_core::MediaSettings,
    gallery_settings: &rd_core::GallerySettings,
) -> Result<(Vec<CapturedLink>, u32), ApiError> {
    // A link whose service is switched off is refused here rather than queued: a row that
    // can never run is worse than a refusal, because nothing in the queue explains it.
    let disabled: Vec<url::Url> = {
        let providers = providers_for(
            &links
                .iter()
                .map(|link| link.url.clone())
                .collect::<Vec<_>>(),
            media_settings,
            gallery_settings,
        );
        links
            .iter()
            .zip(&providers)
            .filter(|(_, provider)| is_service_disabled(settings, provider.as_deref()))
            .map(|(link, _)| link.url.clone())
            .collect()
    };
    let skipped_disabled = u32::try_from(disabled.len()).unwrap_or(u32::MAX);
    if !disabled.is_empty() {
        let (kept, refused): (Vec<_>, Vec<_>) = links
            .into_iter()
            .partition(|link| !disabled.contains(&link.url));
        CapturedLink::discard_bodies(state, &refused).await;
        links = kept;
    }
    if links.is_empty() {
        return Err(ApiError::bad_request(
            "collector.all_links_disabled",
            "Every link needs a transfer service that is switched off",
        ));
    }
    Ok((links, skipped_disabled))
}
