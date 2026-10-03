//! GitHub and GitLab releases as a subscription source (RD-190-13).
//!
//! One request per poll in the common case: the first page of the release list, sent with the
//! validators of the last answer so an unchanged list costs a `304`. A release list that did
//! change can cost a few more — the checksum lists of the newest releases, when they carry one
//! and the forge does not state the digest itself.
//!
//! **The token is the security surface.** It is resolved from the vault for the request that
//! needs it and never stored, logged or put in an address: it travels as an `Authorization`
//! header, which the HTTP client drops on a redirect to another host, so the storage a forge
//! redirects a download to never sees it. The documented rights are the smallest that read
//! releases: a GitHub fine-grained token with "Contents: read", a GitLab token with `read_api`.
//!
//! **A rate limit is not a failure.** GitHub allows a caller without a token sixty requests an
//! hour; a refusal names the time the budget refills, and the adapter reports exactly that as
//! [`RateLimited`] so the poller waits for it instead of backing off and retrying into the
//! same refusal.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rd_core::{FilterReason, GitForge, Subscription, SubscriptionKind};
use url::Url;

use crate::{
    adapter::{DiscoveredItem, PollOutcome, RateLimited, SourceAdapter},
    git_release::{
        ChecksumFile, Release, Repository, checksum_file, parse_checksums, parse_releases, selects,
    },
    indexer::SecretResolver,
};

/// Releases asked for per poll. New releases arrive one at a time; ten covers a burst of
/// them between two polls without making the first poll of a busy repository a crawl.
pub const RELEASES_PER_POLL: u32 = 10;
/// Largest release list read. Release notes are part of it and can be long.
pub const MAX_RELEASE_LIST_BYTES: usize = 4 * 1024 * 1024;
/// Largest checksum list read.
pub const MAX_CHECKSUM_BYTES: usize = 256 * 1024;
/// Most checksum lists one poll fetches, newest releases first.
pub const MAX_CHECKSUM_DOCUMENTS: usize = 4;
/// The REST API version GitHub is asked to answer in.
const GITHUB_API_VERSION: &str = "2022-11-28";
/// Shortest pause after a refusal: a header that says "now" must not cause a retry loop.
const MIN_PAUSE_SECONDS: i64 = 60;
/// Longest pause a header can impose, so a malformed one cannot park a subscription for good.
const MAX_PAUSE_SECONDS: i64 = 24 * 60 * 60;

/// What an API request returned.
#[derive(Clone, Debug, Default)]
pub struct ApiResponse {
    pub status: u16,
    /// Response headers, names in lowercase.
    pub headers: BTreeMap<String, String>,
    /// `None` when the server answered `304`.
    pub body: Option<String>,
}

impl ApiResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Makes the adapter's requests. A trait, like [`crate::FeedFetcher`], so this crate links no
/// transport and the tests answer from recorded documents instead of a forge.
#[async_trait]
pub trait ApiFetcher: Send + Sync {
    /// A `GET` that follows redirects and reads at most `limit` bytes of the body. Any status
    /// is an answer, not an error: what a `403` means is the adapter's to decide.
    async fn get(
        &self,
        url: &Url,
        headers: &[(String, String)],
        limit: usize,
    ) -> anyhow::Result<ApiResponse>;

    /// Where a `GET` of `url` ends up after its redirects, without reading the body; an error
    /// unless the final answer is a success.
    async fn locate(&self, url: &Url, headers: &[(String, String)]) -> anyhow::Result<Url>;
}

/// Polls GitHub and GitLab release lists.
pub struct GitReleaseAdapter {
    fetcher: Arc<dyn ApiFetcher>,
    secrets: Arc<dyn SecretResolver>,
}

impl GitReleaseAdapter {
    #[must_use]
    pub fn new(fetcher: Arc<dyn ApiFetcher>, secrets: Arc<dyn SecretResolver>) -> Self {
        Self { fetcher, secrets }
    }

    async fn token(&self, subscription: &Subscription) -> anyhow::Result<Option<String>> {
        match subscription.secret_ref.as_deref() {
            Some(reference) => Ok(Some(self.secrets.resolve(reference).await?)),
            None => Ok(None),
        }
    }

    /// The SHA-256 of release files, from the checksum files the newest releases carry.
    ///
    /// Best effort: a list that cannot be fetched or read leaves its files without a checksum
    /// and the poll goes on, because the files themselves are still the ones published.
    async fn checksums(
        &self,
        repository: &Repository,
        releases: &[&Release],
        subscription: &Subscription,
        token: Option<&str>,
    ) -> BTreeMap<(String, String), String> {
        let mut sums = BTreeMap::new();
        let mut fetched = 0;
        for release in releases {
            let wanted: Vec<&str> = release
                .assets
                .iter()
                .filter(|asset| asset.sha256.is_none())
                .filter(|asset| selects(&subscription.git_release, &asset.name))
                .map(|asset| asset.name.as_str())
                .collect();
            if wanted.is_empty() {
                continue;
            }
            for asset in &release.assets {
                let Some(kind) = checksum_file(&asset.name) else {
                    continue;
                };
                if let ChecksumFile::Single(covered) = &kind
                    && !wanted.contains(&covered.as_str())
                {
                    continue;
                }
                if fetched >= MAX_CHECKSUM_DOCUMENTS {
                    return sums;
                }
                fetched += 1;
                let (url, headers) = match (token, &asset.api_url) {
                    (Some(token), Some(api_url)) => {
                        (api_url, download_headers(repository.forge, token))
                    }
                    (Some(token), None) if asset.url.host_str() == repository.api.host_str() => {
                        (&asset.url, download_headers(repository.forge, token))
                    }
                    _ => (&asset.url, Vec::new()),
                };
                let text = match self.fetcher.get(url, &headers, MAX_CHECKSUM_BYTES).await {
                    Ok(response) if (200..300).contains(&response.status) => {
                        response.body.unwrap_or_default()
                    }
                    Ok(response) => {
                        tracing::debug!(status = response.status, "checksum list not available");
                        continue;
                    }
                    Err(error) => {
                        tracing::debug!(%error, "checksum list not available");
                        continue;
                    }
                };
                let parsed = parse_checksums(&text);
                match kind {
                    ChecksumFile::Single(covered) => {
                        if let Some(sum) = parsed.get(covered.as_str()).or_else(|| parsed.get("")) {
                            sums.insert((release.id.clone(), covered), sum.clone());
                        }
                    }
                    ChecksumFile::List => {
                        for (name, sum) in parsed {
                            sums.insert((release.id.clone(), name), sum);
                        }
                    }
                }
            }
        }
        sums
    }
}

/// The headers of a release-list request.
fn api_headers(forge: GitForge, token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = match forge {
        GitForge::Github => vec![
            (
                "accept".to_owned(),
                "application/vnd.github+json".to_owned(),
            ),
            (
                "x-github-api-version".to_owned(),
                GITHUB_API_VERSION.to_owned(),
            ),
        ],
        GitForge::Gitlab => vec![("accept".to_owned(), "application/json".to_owned())],
    };
    if let Some(token) = token {
        headers.push(("authorization".to_owned(), format!("Bearer {token}")));
    }
    headers
}

/// The headers that download a file with the token: GitHub's API hands over the bytes, not the
/// file's description, only when asked for `application/octet-stream`.
fn download_headers(forge: GitForge, token: &str) -> Vec<(String, String)> {
    let mut headers = vec![("authorization".to_owned(), format!("Bearer {token}"))];
    if forge == GitForge::Github {
        headers.push(("accept".to_owned(), "application/octet-stream".to_owned()));
        headers.push((
            "x-github-api-version".to_owned(),
            GITHUB_API_VERSION.to_owned(),
        ));
    }
    headers
}

/// When the forge may be asked again, if its answer said anything about that.
///
/// `Retry-After` (seconds or a date) wins, then the reset of the request budget
/// (`X-RateLimit-Reset` on GitHub, `RateLimit-Reset` on GitLab, both Unix seconds). A refusal
/// that names neither waits a minute. A budget that this very answer used up — `Remaining: 0` on
/// a success — pauses until its reset too, so the next poll is not the one that gets refused.
#[must_use]
pub fn rate_limit_pause(
    status: u16,
    headers: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let header = |name: &str| headers.get(name).map(|value| value.trim());
    let retry_after = header("retry-after").and_then(|value| {
        value
            .parse::<i64>()
            .ok()
            // Bounded before it becomes a duration: a header's number is a stranger's.
            .map(|seconds| now + Duration::seconds(seconds.clamp(0, MAX_PAUSE_SECONDS)))
            .or_else(|| {
                DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|instant| instant.with_timezone(&Utc))
            })
    });
    let remaining = header("x-ratelimit-remaining")
        .or_else(|| header("ratelimit-remaining"))
        .and_then(|value| value.parse::<u64>().ok());
    let reset = header("x-ratelimit-reset")
        .or_else(|| header("ratelimit-reset"))
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0));
    let refused =
        status == 429 || (status == 403 && (remaining == Some(0) || retry_after.is_some()));
    let until = if refused {
        Some(
            retry_after
                .or(reset)
                .unwrap_or(now + Duration::seconds(MIN_PAUSE_SECONDS)),
        )
    } else if remaining == Some(0) {
        reset
    } else {
        None
    }?;
    Some(until.clamp(
        now + Duration::seconds(MIN_PAUSE_SECONDS),
        now + Duration::seconds(MAX_PAUSE_SECONDS),
    ))
}

/// Whether an answer is a rate-limit refusal rather than a refusal of access.
fn is_refused_for_rate(status: u16, headers: &BTreeMap<String, String>) -> bool {
    status == 429
        || (status == 403
            && (headers.contains_key("retry-after")
                || headers
                    .get("x-ratelimit-remaining")
                    .or_else(|| headers.get("ratelimit-remaining"))
                    .is_some_and(|value| value.trim() == "0")))
}

#[async_trait]
impl SourceAdapter for GitReleaseAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::GitRelease
    }

    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let options = &subscription.git_release;
        let repository = Repository::parse(&subscription.url, options.forge)?;
        let token = self.token(subscription).await?;
        let mut headers = api_headers(repository.forge, token.as_deref());
        if let Some(etag) = &subscription.etag {
            headers.push(("if-none-match".to_owned(), etag.clone()));
        }
        if let Some(last_modified) = &subscription.last_modified {
            headers.push(("if-modified-since".to_owned(), last_modified.clone()));
        }
        let response = self
            .fetcher
            .get(
                &repository.releases_url(RELEASES_PER_POLL),
                &headers,
                MAX_RELEASE_LIST_BYTES,
            )
            .await?;
        let now = Utc::now();
        let pause = rate_limit_pause(response.status, &response.headers, now);
        if is_refused_for_rate(response.status, &response.headers)
            && let Some(until) = pause
        {
            return Err(RateLimited { until }.into());
        }
        let etag = response.header("etag").map(str::to_owned);
        let last_modified = response.header("last-modified").map(str::to_owned);
        match response.status {
            304 => {
                return Ok(PollOutcome {
                    items: Vec::new(),
                    etag,
                    last_modified,
                    not_modified: true,
                    paused_until: pause,
                });
            }
            200..=299 => {}
            401 => anyhow::bail!("the stored token was refused (HTTP 401)"),
            403 => anyhow::bail!(
                "access refused (HTTP 403): the token may not read this repository's releases"
            ),
            404 => anyhow::bail!(
                "no such repository, or a private one without a token that may read it (HTTP 404)"
            ),
            status => anyhow::bail!("HTTP {status}"),
        }
        let body = response.body.unwrap_or_default();
        let releases = parse_releases(repository.forge, &body, &repository.path)?;
        // Drafts never: a draft is a release its authors have not published, and on GitLab an
        // "upcoming" one whose date has not come. Its files can still change.
        let releases: Vec<&Release> = releases
            .iter()
            .filter(|release| !release.draft && (options.prereleases || !release.prerelease))
            .collect();
        let checksums = self
            .checksums(&repository, &releases, subscription, token.as_deref())
            .await;
        let forge = match repository.forge {
            GitForge::Github => "github",
            GitForge::Gitlab => "gitlab",
        };
        let mut items = Vec::new();
        for release in &releases {
            let sources = options
                .source_archives
                .then_some(release.sources.iter())
                .into_iter()
                .flatten()
                .map(|asset| ("source", asset));
            for (part, asset) in release
                .assets
                .iter()
                .map(|asset| ("asset", asset))
                .chain(sources)
            {
                // With a token, a GitHub file is archived under its API address: the only one
                // a private repository answers, and resolved to a token-free one only when the
                // item is handed over.
                let url = match (&token, &asset.api_url) {
                    (Some(_), Some(api_url)) => api_url.clone(),
                    _ => asset.url.clone(),
                };
                let mut item = DiscoveredItem::new(asset.name.clone(), url);
                // A release's id and the file's: a file added to a published release is new, a
                // file replaced under the same name has a new id and is new as well.
                item.source_id = Some(format!(
                    "{forge}:release:{}:{part}:{}",
                    release.id, asset.id
                ));
                item.published_at = release.published_at;
                item.published_raw.clone_from(&release.published_raw);
                item.attributes
                    .insert("release".to_owned(), release.tag.clone());
                if let Some(size) = asset.size {
                    item.attributes.insert("size".to_owned(), size.to_string());
                }
                if let Some(sum) = asset.sha256.clone().or_else(|| {
                    checksums
                        .get(&(release.id.clone(), asset.name.clone()))
                        .cloned()
                }) {
                    item.attributes.insert("sha256".to_owned(), sum);
                }
                if part == "asset" && !selects(options, &asset.name) {
                    item.refused = Some(FilterReason::AssetNotWanted);
                }
                items.push(item);
            }
        }
        Ok(PollOutcome {
            items,
            etag,
            last_modified,
            not_modified: false,
            paused_until: pause,
        })
    }

    async fn download_address(
        &self,
        subscription: &Subscription,
        url: &Url,
    ) -> anyhow::Result<Url> {
        let repository = Repository::parse(&subscription.url, subscription.git_release.forge)?;
        // Only a file on the forge itself needs the token; a link to anywhere else, and every
        // file of a public repository, is downloaded from the address it was archived under.
        if url.host_str() != repository.api.host_str() {
            return Ok(url.clone());
        }
        let Some(token) = self.token(subscription).await? else {
            return Ok(url.clone());
        };
        let located = self
            .fetcher
            .locate(url, &download_headers(repository.forge, &token))
            .await?;
        if located.host_str() != url.host_str() {
            return Ok(located);
        }
        match repository.forge {
            // GitHub's API address answers a request without the token with the file's
            // description, never with the file.
            GitForge::Github => {
                anyhow::bail!("the forge serves this file only with the token")
            }
            // A GitLab instance serving the file itself: an auth profile for the host carries
            // the token to the download, so the address is handed over as it is.
            GitForge::Gitlab => Ok(url.clone()),
        }
    }
}

#[cfg(test)]
#[path = "git_release_adapter_tests.rs"]
mod git_release_adapter_tests;
