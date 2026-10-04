//! One update check: fetch the manifests a channel reads, verify each against its replay floor.
//!
//! **Where the manifests are.** The stable manifest is an asset of every plain `vX.Y.Z` release,
//! so `releases/latest/download/rdownloader-update-stable.json` always names the newest one —
//! GitHub's `latest` never points at a pre-release, and the address never changes, exactly as
//! the plugin index is found. A beta has no such fixed address: `latest` skips pre-releases, and a
//! moving `beta` tag would be one more thing the release workflow has to push. So the beta
//! channel reads GitHub's release list (`/repos/<repo>/releases`), takes the release of highest
//! version that carries `rdownloader-update-beta.json`, and fetches that asset. The list is
//! unsigned and only *locates* the manifest: the asset address has to lie under the repository's
//! own `releases/download/`, and what is fetched from it verifies under the release root like
//! the stable manifest or is refused. The beta channel reads the stable manifest too, so a beta
//! installation is offered the release that follows its betas.
//!
//! **Floors.** Each channel has its own: the two manifests are separate documents, and switching
//! from beta back to stable must not refuse the stable manifest for a sequence the beta one set.
//! The caller persists [`CheckReport::floors`] before it acts on what verified.

use chrono::{DateTime, Utc};
use rd_sign::TrustStore;
use serde::{Deserialize, Serialize};

use crate::{
    Fetcher,
    manifest::{self, Channel, MAX_MANIFEST_BYTES, UpdateError, UpdateManifest},
    offer::parse_version,
};

/// The repository the official releases are published in.
pub const OFFICIAL_REPOSITORY: &str = "degoya/rDownloader";
/// Releases the beta channel reads per list. Each version publishes two since 1.9.1, the
/// application's and `plugins-vX.Y.Z`, so sixty keep the window at thirty versions.
pub const RELEASE_LIST_PAGE: u32 = 60;
/// Largest release list read. Measured 2026-10-04: a release with the plugins among its ~90
/// assets is ~144 KB in the list, so even sixty of those come to ~8.6 MB.
pub const MAX_RELEASE_LIST_BYTES: u64 = 16 * 1024 * 1024;
const _: () = assert!(RELEASE_LIST_PAGE as u64 * 144 * 1024 < MAX_RELEASE_LIST_BYTES);

/// Where one repository publishes its manifests.
#[derive(Clone, Debug)]
pub struct Sources {
    /// The stable manifest: the newest plain release's asset.
    pub stable: url::Url,
    /// GitHub's release list, where the newest beta manifest is looked up.
    pub releases: url::Url,
    /// Every asset address the list may name starts with this.
    pub download_prefix: String,
}

impl Sources {
    /// The official repository's addresses.
    #[must_use]
    pub fn official() -> Self {
        Self::for_repository(OFFICIAL_REPOSITORY)
    }

    /// The addresses of `repository` (`owner/name`) on github.com.
    fn for_repository(repository: &str) -> Self {
        // A constant repository name: these parse, or the build is broken.
        let parse = |text: String| url::Url::parse(&text).expect("a github.com address parses");
        Self {
            stable: parse(format!(
                "https://github.com/{repository}/releases/latest/download/{}",
                Channel::Stable.file_name()
            )),
            releases: parse(format!(
                "https://api.github.com/repos/{repository}/releases?per_page={RELEASE_LIST_PAGE}"
            )),
            download_prefix: format!("https://github.com/{repository}/releases/download/"),
        }
    }
}

/// The highest manifest sequence accepted per channel.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Floors {
    #[serde(default)]
    pub stable: Option<u64>,
    #[serde(default)]
    pub beta: Option<u64>,
}

impl Floors {
    #[must_use]
    pub fn get(&self, channel: Channel) -> Option<u64> {
        match channel {
            Channel::Stable => self.stable,
            Channel::Beta => self.beta,
        }
    }

    /// Raises `channel`'s floor to `sequence`; never lowers it.
    pub fn raise(&mut self, channel: Channel, sequence: u64) {
        let floor = match channel {
            Channel::Stable => &mut self.stable,
            Channel::Beta => &mut self.beta,
        };
        *floor = Some(floor.map_or(sequence, |known| known.max(sequence)));
    }
}

/// What one check found.
#[derive(Debug)]
pub struct CheckReport {
    /// Every manifest that verified.
    pub manifests: Vec<UpdateManifest>,
    /// The floors after it, to persist before anything acts on [`Self::manifests`].
    pub floors: Floors,
    /// What went wrong, if anything worth reporting did: any refusal of a manifest's integrity,
    /// and otherwise the first failure when nothing verified at all. A stable manifest that is
    /// simply not published yet beside a beta one that verified is not a problem.
    pub problem: Option<UpdateError>,
}

/// Fetches and verifies the manifests `channel` reads.
pub async fn check(
    fetcher: &dyn Fetcher,
    sources: &Sources,
    trust: &TrustStore,
    channel: Channel,
    floors: Floors,
    now: DateTime<Utc>,
) -> CheckReport {
    let mut report = CheckReport {
        manifests: Vec::new(),
        floors,
        problem: None,
    };
    let mut failures = Vec::new();
    let stable = fetch_manifest(fetcher, &sources.stable).await;
    record(
        &mut report,
        &mut failures,
        stable,
        (trust, sources),
        Channel::Stable,
        now,
    );
    if channel == Channel::Beta {
        let beta = match locate_beta(fetcher, sources).await {
            Ok(url) => fetch_manifest(fetcher, &url).await,
            Err(error) => Err(error),
        };
        record(
            &mut report,
            &mut failures,
            beta,
            (trust, sources),
            Channel::Beta,
            now,
        );
    }
    let integrity = failures.iter().position(UpdateError::is_integrity);
    report.problem = match integrity {
        Some(index) => Some(failures.swap_remove(index)),
        None if report.manifests.is_empty() => failures.into_iter().next(),
        None => None,
    };
    report
}

fn record(
    report: &mut CheckReport,
    failures: &mut Vec<UpdateError>,
    fetched: Result<Vec<u8>, UpdateError>,
    (trust, sources): (&TrustStore, &Sources),
    channel: Channel,
    now: DateTime<Utc>,
) {
    let verified = fetched
        .and_then(|bytes| {
            manifest::verify_with(&bytes, trust, channel, report.floors.get(channel), now)
        })
        .and_then(|manifest| pinned(manifest, &sources.download_prefix));
    match verified {
        Ok(manifest) => {
            report.floors.raise(channel, manifest.sequence);
            report.manifests.push(manifest);
        }
        Err(error) => {
            tracing::warn!(channel = channel.as_str(), code = error.code(), %error, "update manifest refused");
            failures.push(error);
        }
    }
}

/// Refuses a verified manifest with an artifact outside the repository's own release downloads
/// (security review 2026-09-30, finding 9): the signature covers every address, and the pin
/// keeps even a signed manifest from sending an installation anywhere else. Compared in the
/// parsed form the download requests, so `..` and its percent-encoded spelling leave nothing out.
pub fn pinned(
    manifest: UpdateManifest,
    download_prefix: &str,
) -> Result<UpdateManifest, UpdateError> {
    let outside = manifest.artifacts.iter().find(|artifact| {
        !url::Url::parse(&artifact.url).is_ok_and(|url| url.as_str().starts_with(download_prefix))
    });
    match outside {
        Some(artifact) => Err(UpdateError::Invalid(format!(
            "the artifact {} is not under {download_prefix}",
            artifact.url
        ))),
        None => Ok(manifest),
    }
}

async fn fetch_manifest(fetcher: &dyn Fetcher, url: &url::Url) -> Result<Vec<u8>, UpdateError> {
    // One byte over the limit is enough to know it is too large.
    match fetcher.fetch(url, MAX_MANIFEST_BYTES as u64 + 1).await {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(UpdateError::NotPublished),
        Err(error) => Err(UpdateError::Fetch(format!("{error:#}"))),
    }
}

async fn locate_beta(fetcher: &dyn Fetcher, sources: &Sources) -> Result<url::Url, UpdateError> {
    let list = fetcher
        .fetch(&sources.releases, MAX_RELEASE_LIST_BYTES)
        .await
        .map_err(|error| UpdateError::Fetch(format!("{error:#}")))?
        .ok_or(UpdateError::NotPublished)?;
    beta_manifest_url(&list, &sources.download_prefix)?.ok_or(UpdateError::NotPublished)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The beta manifest of the highest-versioned release in GitHub's release list, if any carries
/// one under `download_prefix`.
pub fn beta_manifest_url(
    list: &[u8],
    download_prefix: &str,
) -> Result<Option<url::Url>, UpdateError> {
    let releases: Vec<Release> = serde_json::from_slice(list)
        .map_err(|error| UpdateError::Fetch(format!("the release list cannot be read: {error}")))?;
    let newest = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = parse_version(&release.tag_name)?;
            let asset = release.assets.into_iter().find(|asset| {
                asset.name == Channel::Beta.file_name()
                    && asset.browser_download_url.starts_with(download_prefix)
            })?;
            let url = url::Url::parse(&asset.browser_download_url).ok()?;
            (url.scheme() == "https").then_some((version, url))
        })
        .max_by(|(left, _), (right, _)| {
            (left.major, left.minor, left.patch, &left.pre).cmp(&(
                right.major,
                right.minor,
                right.patch,
                &right.pre,
            ))
        });
    Ok(newest.map(|(_, url)| url))
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
