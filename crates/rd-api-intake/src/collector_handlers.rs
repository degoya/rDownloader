//! LinkGrabber intake, packages, ordering and enqueueing.

use anyhow::Context as _;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rd_collector::extract_urls;
use rd_core::{CandidateId, CollectorPackage, CollectorPackageId, LinkCandidateState};
use rd_db::{CollectorPackageChange, MoveTarget, NewCollectorBatch, StoreErrorKind};

use crate::{
    ApiError, AppState,
    collector_intake::{adopt_remote_credentials, is_service_disabled, providers_for},
    dto::{
        CandidateCheckRequest, CandidateMoveRequest, CandidateRenameRequest,
        CandidateReorderRequest, CollectorIntakeRequest, CollectorIntakeResponse,
        CollectorPackageBulkRequest, CollectorPackageEnqueueRequest,
        CollectorPackageReorderRequest, CollectorPackageUpdateRequest, GrabberEntryReorderRequest,
        MessageResponse, PageQuery,
    },
};
use rd_api_core::list_bounds::{paged, validate_bulk};

/// URLs, file names, sizes, package hints, mirror hints, captured requests and vaulted body
/// references, parallel per link.
type SplitLinks = (
    Vec<url::Url>,
    Vec<Option<String>>,
    Vec<Option<rd_core::ByteCount>>,
    Vec<Option<String>>,
    Vec<Option<rd_core::MirrorHint>>,
    Vec<Option<rd_core::CapturedRequest>>,
    Vec<Option<String>>,
);

/// One link on its way into a batch, with the metadata the intake keeps parallel to it.
struct CapturedLink {
    url: url::Url,
    file_name: Option<String>,
    /// Size the source stated, when it did. A crawler reads it out of the folder listing;
    /// nothing else here knows one before the link check has run.
    size: Option<rd_core::ByteCount>,
    /// The package this link's source says it belongs to — the folder a crawler found it in.
    package_hint: Option<String>,
    /// What the source said about this link being one of several copies of the same file
    /// (RD-110-18). Only a crawler or a site rule ever knows this; a pasted link does not.
    mirror: Option<rd_core::MirrorHint>,
    request: Option<rd_core::CapturedRequest>,
    /// `vault://` reference of the encrypted request body, when one was stored.
    body_ref: Option<String>,
    /// Who named the address — decides whether its online check is held to an address rule.
    origin: LinkOrigin,
}

/// Who put a link into an intake (RD-150-03).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkOrigin {
    /// Typed, pasted or sent by the person: checked as it always was.
    Person,
    /// Proposed by an intake parser out of a document — a Metalink's link among them.
    Proposed,
    /// Found by a crawler on a page.
    Crawled,
}

impl LinkOrigin {
    /// Whether the online check of such a link is held to an address rule. A parser's
    /// proposal always is: the document was written by somebody else even when the person
    /// pasted it. A crawler's find only when the page did not come from the person's own hand:
    /// a folder they pointed the crawler at themselves is their own network use.
    fn guarded(self, own_hand: bool) -> bool {
        match self {
            Self::Person => false,
            Self::Proposed => true,
            Self::Crawled => !own_hand,
        }
    }
}

impl CapturedLink {
    /// A link extracted from free text: no client-supplied metadata.
    fn plain(url: url::Url) -> Self {
        Self {
            url,
            file_name: None,
            size: None,
            package_hint: None,
            mirror: None,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Person,
        }
    }

    /// A link an intake parser proposed. It carries a name and nothing else: a plugin never
    /// supplies request metadata, because that would be a credential path it has no claim to.
    fn proposed(url: url::Url, file_name: Option<String>) -> Self {
        Self {
            url,
            file_name: file_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            size: None,
            package_hint: None,
            mirror: None,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Proposed,
        }
    }

    /// A link a folder crawler found (RD-104-03).
    ///
    /// It carries what the folder listing stated — a name, a size and the folder it sat in —
    /// and nothing else. Everything below this point treats it exactly as a pasted link:
    /// the blocklist, the disabled-service refusal, the review and the routing rules all
    /// apply, so a crawler queues nothing by itself.
    fn crawled(link: rd_plugin_ext::CrawledLink) -> Self {
        Self {
            url: link.url,
            file_name: link.file_name,
            size: link
                .size
                .and_then(|size| rd_core::ByteCount::new(size).ok()),
            package_hint: link.package_hint,
            mirror: link.mirror,
            request: None,
            body_ref: None,
            origin: LinkOrigin::Crawled,
        }
    }

    /// Splits into the parallel vectors `NewCollectorBatch` expects.
    fn split(links: Vec<Self>) -> SplitLinks {
        let mut urls = Vec::with_capacity(links.len());
        let mut file_names = Vec::with_capacity(links.len());
        let mut sizes = Vec::with_capacity(links.len());
        let mut package_hints = Vec::with_capacity(links.len());
        let mut mirror_hints = Vec::with_capacity(links.len());
        let mut requests = Vec::with_capacity(links.len());
        let mut body_refs = Vec::with_capacity(links.len());
        for link in links {
            urls.push(link.url);
            file_names.push(link.file_name);
            sizes.push(link.size);
            package_hints.push(link.package_hint);
            mirror_hints.push(link.mirror);
            requests.push(link.request);
            body_refs.push(link.body_ref);
        }
        (
            urls,
            file_names,
            sizes,
            package_hints,
            mirror_hints,
            requests,
            body_refs,
        )
    }

    /// Removes every body this intake vaulted.
    ///
    /// Called when the batch is abandoned after the bodies were already encrypted, so a
    /// rejected or fully-excluded capture cannot leave orphaned ciphertext behind.
    async fn discard_bodies(state: &AppState, links: &[Self]) {
        for link in links {
            if let Some(reference) = &link.body_ref
                && let Err(error) = state.secrets.remove(reference).await
            {
                tracing::warn!(%error, "could not remove an unused captured request body");
            }
        }
    }
}

#[utoipa::path(post, path = "/api/v1/collector/batches", tag = "collector", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse)))]
pub async fn collector_intake(
    State(state): State<AppState>,
    Json(request): Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    let response = collector_intake_inner(&state, request).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn collector_intake_inner(
    state: &AppState,
    request: CollectorIntakeRequest,
) -> Result<CollectorIntakeResponse, ApiError> {
    // Structured links carry per-link metadata; free text keeps the legacy paste path.
    if request.links.len() > rd_core::MAX_CAPTURE_LINKS {
        return Err(crate::capture_sanitize::links_limit(
            rd_core::MAX_CAPTURE_LINKS,
        ));
    }
    let mut links: Vec<CapturedLink> = Vec::new();
    for (index, link) in request.links.into_iter().enumerate() {
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
            file_name: link
                .file_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            size: None,
            package_hint: None,
            mirror: None,
            request,
            body_ref,
            origin: LinkOrigin::Person,
        });
    }
    let text = request.text.as_deref().unwrap_or_default();
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
    let (mut links, skipped): (Vec<_>, Vec<_>) = links.into_iter().partition(|link| {
        !link
            .url
            .host_str()
            .is_some_and(|host| crate::collector_exclusions::is_excluded(&excluded, host))
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
    // A link whose service is switched off is refused here rather than queued: a row that
    // can never run is worse than a refusal, because nothing in the queue explains it.
    let settings = crate::settings_store::read_settings(state).await?;
    let media_settings = state.media_settings.read().await.clone();
    let gallery_settings = state.gallery_settings.read().await.clone();
    let disabled: Vec<url::Url> = {
        let providers = providers_for(
            &links
                .iter()
                .map(|link| link.url.clone())
                .collect::<Vec<_>>(),
            &media_settings,
            &gallery_settings,
        );
        links
            .iter()
            .zip(&providers)
            .filter(|(_, provider)| is_service_disabled(&settings, provider.as_deref()))
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
            package_name: request
                .package_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            password: request
                .password
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
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
        skipped_excluded: u32::try_from(skipped.len()).unwrap_or(u32::MAX),
        skipped_disabled,
        crawled_found: crawled.found,
        crawled_dropped: crawled.dropped,
    })
}

/// Replaces every link an installed crawler claims with the files behind it (RD-104-03).
///
/// Three rules, and each of them is a refusal rather than a feature:
///
/// - **A crawled link is never crawled again.** One pass, no recursion: a crawler that
///   answered with another folder address of its own would otherwise be walked for ever, and
///   the plugin's own walk is where a tree belongs.
/// - **A captured browser request is never crawled.** It is one intercepted download with
///   headers and a body behind it, not a share; expanding it would leave a vaulted body with
///   no owner.
/// - **A folder that produced nothing says so.** An empty or unreachable folder ends as a
///   refusal carrying the plugin's stable code, so the interface can say it in the language
///   the person reads. Silently dropping the link is the defect this replaced.
/// - **Nothing a crawler returns is taken on trust.** Every address it hands back passes the
///   verdict in [`crate::collector_crawl_verdict`] before a row exists for it: claimed by a
///   resolver, confirmed as file content by a probe, or refused because the probe answered
///   with a page (RD-110-07). A rule whose pattern reached one element too far used to put
///   the board page itself into the list, and the queue then stored that page under the name
///   of an episode. An address the probe could not reach at all is **kept** -- see that
///   module, and RD-101-06.
async fn expand_crawled_links(
    state: &AppState,
    links: Vec<CapturedLink>,
    guard: Option<&rd_http::AddressPolicy>,
) -> Result<Crawled, ApiError> {
    if state.crawlers.is_empty() {
        return Ok(Crawled::untouched(links));
    }
    let accounts = crawl_accounts(state).await;
    let media_settings = state.media_settings.read().await.clone();
    let gallery_settings = state.gallery_settings.read().await.clone();
    let mut expanded: Vec<CapturedLink> = Vec::with_capacity(links.len());
    let mut refusal: Option<(String, String)> = None;
    let mut found_total: usize = 0;
    let mut dropped: usize = 0;
    let mut crawled_any = false;
    // One budget for the whole pass, not one per address: this runs inside the intake
    // request, and the per-address timeout alone would let a folder of five hundred stalled
    // links hold it open for hours. What the budget does not reach is kept unproven.
    let probe_deadline = std::time::Instant::now() + crate::collector_crawl_verdict::PROBE_BUDGET;
    for link in links {
        if link.request.is_some() || link.body_ref.is_some() {
            expanded.push(link);
            continue;
        }
        match state.crawlers.expand(&link.url, &accounts).await {
            rd_plugin_ext::CrawlOutcome::NotClaimed => expanded.push(link),
            rd_plugin_ext::CrawlOutcome::Links(found) => {
                crawled_any = true;
                found_total += found.len();
                // Before the links are kept: the credential a protected share needs, so the
                // queue reaches the files with the same login the crawl read them with.
                if let Err(error) = adopt_share_login(state, &link.url, &found).await {
                    // Named without its cause, which could quote the credential: a share
                    // that ends up without a profile fails visibly at the first download.
                    tracing::warn!(
                        error = %rd_core::redact_text(&error.to_string()),
                        "a protected share's login could not be stored"
                    );
                }
                for crawled in found {
                    if expanded.iter().any(|kept| kept.url == crawled.url) {
                        continue;
                    }
                    let verdict = crate::collector_crawl_verdict::verdict(
                        state,
                        &crawled.url,
                        &media_settings,
                        &gallery_settings,
                        probe_deadline,
                        guard,
                    )
                    .await;
                    if verdict.keeps_the_link() {
                        if verdict == crate::collector_crawl_verdict::CrawlVerdict::Unconfirmed {
                            tracing::debug!(
                                address = %for_log(&crawled.url),
                                "a crawled address answered nothing and was kept unconfirmed"
                            );
                        }
                        expanded.push(CapturedLink::crawled(crawled));
                        continue;
                    }
                    dropped += 1;
                    if let Some(code) = verdict.code() {
                        tracing::info!(
                            address = %for_log(&crawled.url),
                            code,
                            "a crawled address is not a file and was not added"
                        );
                        refusal
                            .get_or_insert_with(|| (code.to_owned(), verdict.message().to_owned()));
                    }
                }
            }
            rd_plugin_ext::CrawlOutcome::Refused { code, message } => {
                // Without the fragment: that is where a share password rides, and a wrong
                // one is exactly the case this line is written for.
                tracing::info!(address = %for_log(&link.url), %code, "a crawler could not open a folder");
                refusal.get_or_insert((code, message));
            }
        }
    }
    let crawled = Crawled {
        links: expanded,
        found: u32::try_from(found_total).unwrap_or(u32::MAX),
        dropped: u32::try_from(dropped).unwrap_or(u32::MAX),
    };
    // A refusal is only fatal when nothing else survived: a batch of ten links must not fail
    // over one folder that has gone, and a single folder link that produced nothing must not
    // end as "no links found", which says nothing about what actually happened.
    if crawled.links.is_empty()
        && let Some((code, message)) = refusal
    {
        // The numbers ride along even here, because "the crawler found 40 links and none of
        // them is a file" and "the folder is empty" are different news and used to read the
        // same.
        return Err(ApiError::bad_request_owned(code, message)
            .with_param("found", crawled.found)
            .with_param("dropped", crawled.dropped));
    }
    if crawled_any {
        tracing::info!(
            links = crawled.links.len(),
            found = crawled.found,
            dropped = crawled.dropped,
            "a folder address was expanded"
        );
    }
    Ok(crawled)
}

/// What the crawl pass produced: the links that survived it, and the count it owes the caller.
///
/// The numbers are the point. A rule whose container pattern greys one element too many used
/// to look like an empty page -- twelve of forty links quietly gone and nothing saying so.
struct Crawled {
    links: Vec<CapturedLink>,
    /// Addresses the crawlers handed back, before the verdict looked at any of them.
    found: u32,
    /// How many of those were refused. `found - dropped` reached the list.
    dropped: u32,
}

impl Crawled {
    /// The links of an intake where no crawler ran at all.
    fn untouched(links: Vec<CapturedLink>) -> Self {
        Self {
            links,
            found: 0,
            dropped: 0,
        }
    }
}

/// An address as a log line may carry it: no fragment, and nothing else it hides either.
///
/// A share password rides in the fragment, which is the one part of a URL that never reaches
/// a server -- and would reach a log file, where it outlives the download by months.
fn for_log(url: &url::Url) -> String {
    let mut shown = url.clone();
    shown.set_fragment(None);
    rd_core::redact_url(&shown)
}

/// Longest share password taken from an address, matching what a profile may hold.
const MAX_SHARE_PASSWORD: usize = rd_core::MAX_AUTH_SECRET;

/// Stores the credential a protected share's files need, as an auth profile (RD-108-07).
///
/// **Two decisions live here.** A share password reaches the crawler as the fragment of the
/// pasted address, because `crawl` runs inside the sandbox under a fuel and time budget and
/// has nobody to ask -- and because a fragment is the one part of a URL that is never sent to
/// a server. And a protected share is *not* an account: an account is a per-provider login
/// with a life of its own, while a share password authenticates one share and means nothing
/// anywhere else. What fits it is an auth profile, whose secret is a `vault://` reference and
/// whose scope is a host and a path prefix -- so the queue, which knows nothing about shares,
/// picks the credential up by matching the address it is about to fetch.
///
/// The password is taken from the fragment, encrypted, and never written anywhere else: not
/// into the candidate's address, not into the profile row, not into a REST answer, not into a
/// log line.
async fn adopt_share_login(
    state: &AppState,
    crawled: &url::Url,
    links: &[rd_plugin_ext::CrawledLink],
) -> anyhow::Result<()> {
    let Some(password) = share_password(crawled) else {
        return Ok(());
    };
    let Some(login) = rd_plugin_ext::share_login(links) else {
        return Ok(());
    };
    // The profile of a share crawled before is rewritten rather than duplicated: pasting the
    // same share twice must not leave two credentials behind for one folder.
    let existing = state
        .database
        .list_auth_profiles()
        .await?
        .into_iter()
        .find(|profile| {
            profile.scope == login.scope
                && profile.method == rd_core::AuthMethod::Basic
                && profile.username.as_deref() == Some(login.username.as_str())
        });
    let secret_ref = state.secrets.put_string(password).await?;
    let name = format!(
        "{}{}",
        login.scope.host,
        login.scope.path_prefix.as_deref().unwrap_or("/")
    );
    let orphaned = match existing {
        Some(profile) => {
            let (_, orphaned) = state
                .database
                .update_auth_profile(
                    profile.id,
                    rd_db::UpdateAuthProfile {
                        name: profile.name,
                        scope: login.scope,
                        method: rd_core::AuthMethod::Basic,
                        enabled: true,
                        expires_at: profile.expires_at,
                        username: Some(login.username),
                        secret_ref: Some(secret_ref),
                        certificate_ref: profile.certificate_ref,
                    },
                )
                .await?;
            orphaned
        }
        None => {
            state
                .database
                .create_auth_profile(rd_db::NewAuthProfile {
                    name,
                    scope: login.scope,
                    method: rd_core::AuthMethod::Basic,
                    origin: rd_core::AuthOrigin::Manual,
                    enabled: true,
                    expires_at: None,
                    username: Some(login.username),
                    secret_ref: Some(secret_ref),
                    certificate_ref: None,
                })
                .await?;
            Vec::new()
        }
    };
    for reference in orphaned {
        // Left behind, it is an unreferenced secret in the vault, not a broken package; said,
        // so it can be found (audit 1.9.1, API-13).
        if let Err(error) = state.secrets.remove(&reference).await {
            tracing::warn!(
                error = %format!("{error:#}"),
                "a replaced credential could not be removed from the vault"
            );
        }
    }
    Ok(())
}

/// The share password somebody appended to an address, decoded exactly as the plugin that
/// sent it decodes it.
fn share_password(crawled: &url::Url) -> Option<String> {
    let password = percent_encoding::percent_decode_str(crawled.fragment()?)
        .decode_utf8_lossy()
        .trim()
        .to_owned();
    if password.is_empty() || password.len() > MAX_SHARE_PASSWORD {
        return None;
    }
    Some(password)
}

/// The account each crawler's provider runs as: enabled, and holding a credential.
///
/// Which account a crawl runs as is the application's decision. A plugin naming one would be
/// naming an account it has no business knowing, so it names its provider and gets whatever
/// this finds — or nothing, which is right for a public share.
async fn crawl_accounts(state: &AppState) -> std::collections::HashMap<String, rd_core::AccountId> {
    let providers = state.crawlers.providers();
    if providers.is_empty() {
        return std::collections::HashMap::new();
    }
    let Ok(accounts) = state.database.list_accounts().await else {
        return std::collections::HashMap::new();
    };
    let mut chosen = std::collections::HashMap::new();
    for account in accounts {
        if !account.enabled || !(account.has_secret || account.has_cookies) {
            continue;
        }
        let provider = account.provider.to_ascii_lowercase();
        if providers.contains(&provider) {
            chosen.entry(provider).or_insert(account.id);
        }
    }
    chosen
}

#[utoipa::path(post, path = "/api/v1/capture/batches", tag = "capture", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "Invalid links or request metadata"), (status = 401, description = "Capture token missing or revoked")))]
pub async fn capture_intake(
    state: State<AppState>,
    request: Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    collector_intake(state, request).await
}

/// Every LinkGrabber package in its list order; `limit`/`offset` cut a page out (API-15).
#[utoipa::path(get, path = "/api/v1/collector/packages", tag = "collector", params(PageQuery), responses((status = 200, body = [rd_core::CollectorPackage], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_collector_packages(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<CollectorPackage>>), ApiError> {
    let window = page.window()?;
    Ok(paged(
        window,
        state.database.list_collector_packages().await?,
    ))
}

#[utoipa::path(patch, path = "/api/v1/collector/packages/{id}", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), request_body = CollectorPackageUpdateRequest, responses((status = 200, body = rd_core::CollectorPackage), (status = 404)))]
pub async fn update_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
    Json(request): Json<CollectorPackageUpdateRequest>,
) -> Result<Json<CollectorPackage>, ApiError> {
    let change = package_change(
        &state,
        request.name,
        request.category_id,
        request.clear_category,
        request.priority,
        request.password,
        request.clear_password,
        crate::postprocess_handlers::postprocess_change(
            request.postprocess_level,
            request.clear_postprocess_level,
            request.script,
            request.clear_script,
        )?,
    )
    .await?;
    state
        .database
        .update_collector_packages(vec![id], change)
        .await?
        .pop()
        .map(Json)
        .ok_or_else(crate::error_codes::package_not_found)
}

#[utoipa::path(post, path = "/api/v1/collector/packages/bulk", tag = "collector", request_body = CollectorPackageBulkRequest, responses((status = 200, body = [rd_core::CollectorPackage])))]
pub async fn bulk_update_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageBulkRequest>,
) -> Result<Json<Vec<CollectorPackage>>, ApiError> {
    validate_bulk(request.ids.len())?;
    let change = package_change(
        &state,
        None,
        request.category_id,
        request.clear_category,
        request.priority,
        None,
        false,
        crate::postprocess_handlers::postprocess_change(
            request.postprocess_level,
            request.clear_postprocess_level,
            request.script,
            request.clear_script,
        )?,
    )
    .await?;
    Ok(Json(
        state
            .database
            .update_collector_packages(request.ids, change)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/reorder", tag = "collector", request_body = CollectorPackageReorderRequest, responses((status = 200, body = MessageResponse)))]
pub async fn reorder_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    state
        .database
        .reorder_collector_packages(request.ids)
        .await?;
    Ok(message("collector.order_saved", "Order saved"))
}

/// The manual order of the LinkGrabber list, both kinds in one sequence.
///
/// `POST /api/v1/collector/packages/reorder` can only number the collector packages, so an NZB
/// import sat wherever its creation time put it and could not be dragged at all. This endpoint
/// takes the mixed list and writes one sequence over both tables in a single transaction.
///
/// The body is a *slice* of that order: the entries that moved, and the `after` entry they were
/// dropped behind. Everything else keeps its relative order, so a drag costs the same two entries
/// whether the row sits at the top of the list or three thousand rows down. That is why the
/// ordinary bulk bound fits: it limits how many rows one gesture may move, not how deep into the
/// list the gesture reached. An earlier version had no anchor, so the list could only describe a
/// prefix and a move below entry 500 was refused for its depth alone.
#[utoipa::path(post, path = "/api/v1/collector/entries/reorder", tag = "collector", request_body = GrabberEntryReorderRequest, responses((status = 200, body = MessageResponse), (status = 400), (status = 404), (status = 409)))]
pub async fn reorder_grabber_entries(
    State(state): State<AppState>,
    Json(request): Json<GrabberEntryReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.entries.len())?;
    // The anchor is spliced *behind*, so it cannot also be one of the entries being moved: it
    // leaves the sequence together with them, and nothing is left to splice behind. The store
    // sees only an anchor it cannot find at that point; here both halves of the request are
    // still visible, so this is the one place the mistake can be named for what it is.
    if let Some(after) = request.after
        && request.entries.contains(&after)
    {
        return Err(ApiError::bad_request(
            "collector.entry_anchor_listed",
            "The anchor entry cannot be one of the entries being moved",
        ));
    }
    state
        .database
        .reorder_grabber_entries(request.entries, request.after)
        .await
        .map_err(|error| {
            crate::error_codes::store_error(
                &error,
                "collector.entry_not_found",
                "LinkGrabber entry not found",
                rd_db::StoreErrorKind::Duplicate,
                "collector.entry_duplicate",
                "LinkGrabber entry listed more than once",
            )
        })?;
    Ok(message("collector.order_saved", "Order saved"))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/regroup", tag = "collector", responses((status = 200, body = MessageResponse)))]
pub async fn regroup_collector_packages(
    State(state): State<AppState>,
) -> Result<Json<MessageResponse>, ApiError> {
    let batches: Vec<_> = state
        .database
        .list_collector_packages()
        .await?
        .into_iter()
        .map(|package| package.batch_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    state.database.regroup_collector_batches(batches).await?;
    Ok(message(
        "collector.packages_regrouped",
        "Packages regrouped",
    ))
}

/// The standing mirror preference (RD-110-19).
///
/// Its own route rather than a field of the settings document: it is set from the LinkGrabber
/// toolbar while the settings page may be open elsewhere, and the settings document is written
/// back whole.
#[utoipa::path(get, path = "/api/v1/collector/mirror-preference", tag = "collector", responses((status = 200, body = rd_core::MirrorPreference)))]
pub async fn get_mirror_preference(
    State(state): State<AppState>,
) -> Result<Json<rd_core::MirrorPreference>, ApiError> {
    Ok(Json(state.database.mirror_preference().await?))
}

/// Stores the standing mirror preference and re-chooses every group under it.
///
/// The re-choice happens here rather than in the browser because the choice is what the queue
/// will fetch: a preference the interface has applied to its own rows while the stored rows
/// still name the previous mirror is how somebody queues the one they moved away from.
#[utoipa::path(put, path = "/api/v1/collector/mirror-preference", tag = "collector", request_body = rd_core::MirrorPreference, responses((status = 200, body = rd_core::MirrorPreference)))]
pub async fn put_mirror_preference(
    State(state): State<AppState>,
    Json(request): Json<rd_core::MirrorPreference>,
) -> Result<Json<rd_core::MirrorPreference>, ApiError> {
    let preference = rd_core::MirrorPreference {
        quality: trimmed_facet(request.quality),
        language: trimmed_facet(request.language),
        hoster: trimmed_facet(request.hoster),
        hidden_hosters: normalized_hosters(request.hidden_hosters),
    };
    if preference.hidden_hosters.len() > MAX_HIDDEN_HOSTERS {
        return Err(ApiError::bad_request(
            "collector.hidden_hosters_count",
            "At most 256 hosters can be hidden",
        )
        .with_param("max", MAX_HIDDEN_HOSTERS));
    }
    // A host name has at most 253 characters; anything longer is not one of the list's hosters.
    if preference
        .hidden_hosters
        .iter()
        .any(|hoster| hoster.chars().count() > MAX_HOSTER)
    {
        return Err(ApiError::bad_request(
            "collector.hidden_hoster_length",
            "A hidden hoster must be at most 253 characters",
        )
        .with_param("max", MAX_HOSTER));
    }
    for value in [
        &preference.quality,
        &preference.language,
        &preference.hoster,
    ]
    .into_iter()
    .flatten()
    {
        // A facet is a token off a release name or a host; nothing legitimate is longer, and
        // the value is compared against every candidate of every package on every regroup.
        if value.chars().count() > MAX_FACET {
            return Err(ApiError::bad_request(
                "collector.mirror_facet_length",
                "A mirror facet must be at most 64 characters",
            )
            .with_param("max", MAX_FACET));
        }
    }
    state
        .database
        .set_mirror_preference(preference.clone())
        .await?;
    Ok(Json(preference))
}

/// Makes one link its mirror group's chosen mirror, or releases that choice.
///
/// A pin outranks the preference and survives a regroup, which is what makes it the per-package
/// way out of a standing default.
#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/mirror", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn pin_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    set_mirror_pin(&state, id, true).await
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates/{id}/mirror", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn release_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    set_mirror_pin(&state, id, false).await
}

/// Takes a proposed mirror group apart, so its links stand on their own again (RD-110-34).
///
/// Only a proposal: a group a page declared, or one two links corroborated with a matching
/// size, is refused here rather than asked about. A contradiction against those two is a
/// finding about the source, and the answer to it is to fix the rule, not to click the group
/// away in one package and meet it again on the next page.
#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/mirror/dissolve", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn dissolve_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    match state.database.dissolve_mirror_group(id).await? {
        rd_db::MirrorDissolve::Dissolved => Ok(message(
            "collector.mirror_dissolved",
            "Mirror group dissolved",
        )),
        rd_db::MirrorDissolve::NotProposed => Err(ApiError::conflict(
            "collector.mirror_group_not_proposed",
            "This mirror group is not a proposal and cannot be dissolved",
        )),
        rd_db::MirrorDissolve::NotGrouped => Err(ApiError::not_found(
            "collector.mirror_not_grouped",
            "This link is not part of a mirror group",
        )),
    }
}

/// Longest a single facet value may be.
const MAX_FACET: usize = 64;
/// The hosters one LinkGrabber can hide at once (RD-130-21); far more than a list ever holds.
const MAX_HIDDEN_HOSTERS: usize = 256;
/// The longest host name DNS allows.
const MAX_HOSTER: usize = 253;

/// An empty facet is no facet: a select that was cleared sends `""`, and storing that would
/// make the preference match nothing and hide the whole list.
fn trimmed_facet(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Hidden hosters in the form the grouping compares them in (RD-130-21): the shared
/// [`rd_core::host_key`] (trimmed, lowercased, without a trailing dot or a leading `www.`;
/// audit 1.9.1, INTAKE-11) — the reduction `hosterOf` makes in the interface — each once and
/// sorted, so the stored list reads the same however it was sent.
fn normalized_hosters(values: Vec<String>) -> Vec<String> {
    let hosters: std::collections::BTreeSet<String> = values
        .into_iter()
        .filter_map(|value| {
            let value = rd_core::host_key(&value);
            (!value.is_empty()).then_some(value)
        })
        .collect();
    hosters.into_iter().collect()
}

async fn set_mirror_pin(
    state: &AppState,
    id: CandidateId,
    pinned: bool,
) -> Result<Json<MessageResponse>, ApiError> {
    if state.database.set_mirror_pin(id, pinned).await? {
        return Ok(message(
            if pinned {
                "collector.mirror_pinned"
            } else {
                "collector.mirror_released"
            },
            if pinned {
                "Mirror chosen"
            } else {
                "Mirror choice released"
            },
        ));
    }
    Err(ApiError::not_found(
        "collector.mirror_not_grouped",
        "This link is not part of a mirror group",
    ))
}

#[utoipa::path(delete, path = "/api/v1/collector/packages/{id}", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_collector_package(id)
        .await
        .map_err(|error| {
            crate::error_codes::store_error(
                &error,
                "package.not_found",
                "Package not found",
                StoreErrorKind::Busy,
                "collector.package_busy",
                "Package is being checked or enqueued",
            )
        })?;
    crate::torrent_intake::prune_checked_torrents(&state).await;
    Ok(message(
        "collector.package_removed",
        "Package removed from the LinkGrabber",
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/{id}/enqueue", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path)), responses((status = 201, body = rd_core::DownloadPackage), (status = 409)))]
pub async fn enqueue_collector_package(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
) -> Result<(StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let outcome = crate::collector_enqueue::enqueue_package(&state, id, false, None).await?;
    Ok((StatusCode::CREATED, Json(outcome.package)))
}

#[utoipa::path(post, path = "/api/v1/collector/packages/enqueue", tag = "collector", request_body = CollectorPackageEnqueueRequest, responses((status = 201, body = crate::dto::CollectorEnqueueBatchResponse)))]
pub async fn enqueue_collector_packages(
    State(state): State<AppState>,
    Json(request): Json<CollectorPackageEnqueueRequest>,
) -> Result<(StatusCode, Json<crate::dto::CollectorEnqueueBatchResponse>), ApiError> {
    validate_bulk(request.ids.len())?;
    let mut created = Vec::with_capacity(request.ids.len());
    let mut free_download_files = 0u32;
    let mut failed = 0u32;
    let mut first_error: Option<ApiError> = None;
    for id in request.ids {
        let only = request.candidate_ids.clone();
        match crate::collector_enqueue::enqueue_package(&state, id, request.paused, only).await {
            Ok(outcome) => {
                created.push(outcome.package);
                free_download_files += outcome.free_download_files;
            }
            Err(error) => {
                tracing::warn!(package_id = %id, message = error.message(), "enqueue failed");
                failed += 1;
                first_error.get_or_insert(error);
            }
        }
    }
    if created.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(crate::dto::CollectorEnqueueBatchResponse {
            first_error: first_error.map(|error| error.message().to_owned()),
            created,
            failed,
            free_download_files,
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/move", tag = "collector", request_body = CandidateMoveRequest, responses((status = 200, body = rd_core::CollectorPackage)))]
pub async fn move_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateMoveRequest>,
) -> Result<Json<CollectorPackage>, ApiError> {
    validate_bulk(request.ids.len())?;
    let target = match (request.package_id, request.new_package_name) {
        (Some(id), _) => MoveTarget::Existing(id),
        (None, Some(name)) if !name.trim().is_empty() => MoveTarget::New { name },
        _ => {
            return Err(ApiError::bad_request(
                "collector.move_target_missing",
                "Target package or new package name is missing",
            ));
        }
    };
    Ok(Json(
        state.database.move_candidates(request.ids, target).await?,
    ))
}

/// Writes the manual link order inside one package.
///
/// The list has to be exactly the package's links, each of them once: the store hands out the
/// positions 1..n from it, and its `UPDATE` is fenced by `package_id`, so a foreign id used to
/// be a silent no-op that still answered "Order saved". `/api/v1/downloads/reorder` applies the
/// same rule to the download queue.
#[utoipa::path(post, path = "/api/v1/collector/candidates/reorder", tag = "collector", request_body = CandidateReorderRequest, responses((status = 200, body = MessageResponse), (status = 400)))]
pub async fn reorder_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    let members: Vec<rd_core::CandidateId> = state
        .database
        .list_candidates()
        .await?
        .into_iter()
        .filter(|candidate| candidate.package_id == Some(request.package_id))
        .map(|candidate| candidate.id)
        .collect();
    crate::error_codes::validate_reorder(&members, &request.ids)?;
    state
        .database
        .reorder_candidates(request.package_id, request.ids)
        .await?;
    Ok(message("collector.order_saved", "Order saved"))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/check", tag = "collector", request_body = CandidateCheckRequest, responses((status = 202, body = MessageResponse)))]
pub async fn check_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateCheckRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let ids = match request.ids {
        Some(ids) if !ids.is_empty() => ids,
        _ => state
            .database
            .list_candidates()
            .await?
            .into_iter()
            .filter(|candidate| {
                !matches!(
                    candidate.state,
                    LinkCandidateState::Checking | LinkCandidateState::Resolving
                )
            })
            .map(|candidate| candidate.id)
            .collect(),
    };
    let count = ids.len();
    state.link_check.check(ids).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "collector.check_started",
                format!("Check started for {count} link(s)"),
            )
            .with_count(count),
        ),
    ))
}

#[utoipa::path(patch, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path)), request_body = CandidateRenameRequest, responses((status = 200, body = rd_core::LinkCandidate), (status = 404)))]
pub async fn rename_candidate(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<CandidateRenameRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    if let Some(variant) = request.media_variant.as_deref().map(str::trim) {
        let candidate = state
            .database
            .set_candidate_media_variant(id, variant.to_owned())
            .await
            .map_err(|error| match rd_db::store_kind(&error) {
                Some(StoreErrorKind::NotFound) => {
                    ApiError::not_found("collector.candidate_not_found", "Link not found")
                }
                Some(StoreErrorKind::UnknownMediaVariant) => {
                    ApiError::bad_request("media.variant_unknown", "Unknown media variant")
                }
                Some(StoreErrorKind::NoMediaMetadata) => {
                    ApiError::bad_request("media.not_media_link", "This link has no media variants")
                }
                _ => ApiError::conflict("collector.candidate_busy", "Link is busy"),
            })?;
        if request.file_name.is_none() {
            return Ok(Json(candidate));
        }
    }
    let Some(file_name) = request.file_name.as_deref() else {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    };
    let file_name = rd_files::sanitize_file_name(file_name.trim());
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "collector.file_name_length",
            "File name must be between 1 and 255 characters",
        )
        .with_param("max", 255));
    }
    state
        .database
        .set_candidate_file_name(id, file_name)
        .await
        .map(Json)
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "collector.candidate_not_found",
                "Link not found",
            )
        })
}

#[allow(clippy::too_many_arguments)]
async fn package_change(
    state: &AppState,
    name: Option<String>,
    category_id: Option<rd_core::CategoryId>,
    clear_category: bool,
    priority: Option<rd_core::DownloadPriority>,
    password: Option<String>,
    clear_password: bool,
    postprocess: crate::postprocess_handlers::PostprocessChange,
) -> Result<CollectorPackageChange, ApiError> {
    let name = match name {
        Some(value) => Some(rd_api_core::input_checks::required_text(
            &value,
            rd_api_core::input_checks::TextLimit::Chars(200),
            "package.name_length",
            "Package name must be between 1 and 200 characters",
        )?),
        None => None,
    };
    if let Some(id) = category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let category = if clear_category {
        Some(None)
    } else {
        category_id.map(Some)
    };
    let password = match (password, clear_password) {
        (_, true) => Some(None),
        (Some(value), false) => Some(Some(rd_api_core::input_checks::required_text(
            &value,
            rd_api_core::input_checks::TextLimit::Chars(1024),
            "package.password_length",
            "Archive password must be between 1 and 1024 characters",
        )?)),
        (None, false) => None,
    };
    if name.is_none()
        && category.is_none()
        && priority.is_none()
        && password.is_none()
        && postprocess.level.is_none()
        && postprocess.script.is_none()
    {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    }
    Ok(CollectorPackageChange {
        name,
        category_id: category,
        priority,
        password,
        postprocess_level: postprocess.level,
        script: postprocess.script,
    })
}

fn message(code: &str, text: &str) -> Json<MessageResponse> {
    Json(MessageResponse::new(code, text))
}

#[cfg(test)]
mod tests {
    use super::{LinkOrigin, for_log, share_password};

    /// RD-150-03: what a document or a stranger's page proposed is checked under the address
    /// rule; what the person gave, and a folder they pointed the crawler at, is not.
    #[test]
    fn only_links_somebody_else_proposed_are_checked_under_the_address_rule() {
        for own_hand in [true, false] {
            assert!(!LinkOrigin::Person.guarded(own_hand));
            assert!(LinkOrigin::Proposed.guarded(own_hand));
        }
        assert!(!LinkOrigin::Crawled.guarded(true));
        assert!(LinkOrigin::Crawled.guarded(false));
    }

    /// The fragment of a crawled address is a share password (RD-108-07), and a log line is
    /// the place a credential survives longest. It never goes in one.
    #[test]
    fn a_share_password_never_reaches_a_log_line() {
        let address = url::Url::parse("https://cloud.example.org/s/QxT7bK2mNp9wZr4#s3cret")
            .expect("an address");
        let shown = for_log(&address);
        assert!(!shown.contains("s3cret"), "{shown}");
        assert_eq!(shown, "https://cloud.example.org/s/QxT7bK2mNp9wZr4");
        // Whatever else a URL hides is still redacted; the fragment is only the addition.
        let signed = url::Url::parse("https://cloud.example.org/f?X-Amz-Signature=abc#s3cret")
            .expect("an address");
        let shown = for_log(&signed);
        assert!(
            !shown.contains("s3cret") && !shown.contains("abc"),
            "{shown}"
        );
    }

    #[test]
    fn a_password_is_read_from_the_fragment_the_way_the_plugin_reads_it() {
        let password =
            |address: &str| share_password(&url::Url::parse(address).expect("an address"));
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#let%20me%20in").as_deref(),
            Some("let me in")
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4"),
            None
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#"),
            None
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#%20%20"),
            None
        );
        let long = "x".repeat(super::MAX_SHARE_PASSWORD + 1);
        assert_eq!(
            password(&format!("https://cloud.example.org/s/Qx#{long}")),
            None
        );
    }
}
