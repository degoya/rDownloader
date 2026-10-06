//! Reading a GitHub or GitLab release list, and choosing its files (RD-190-13).
//!
//! The pure half of the git-release adapter: which API a repository address belongs to, what
//! the two forges answer, which assets the subscription's options select, and what a checksum
//! list a release carries says. The requests are [`crate::GitReleaseAdapter`]'s.
//!
//! The two answers differ more than they look. A GitHub release has an id, a draft and a
//! pre-release flag, and assets with ids, sizes and (lately) a SHA-256 digest. A GitLab release
//! has none of those: it is named by its tag, an unpublished one is an "upcoming" release, a
//! pre-release is only recognisable by its tag, and its files are links with ids that may point
//! anywhere. Both carry source archives. One [`Release`] shape holds what both say, so the
//! adapter decides once.

use chrono::{DateTime, Utc};
use rd_core::GitForge;
use serde::Deserialize;
use url::Url;

mod assets;

pub use assets::{
    ChecksumFile, architecture_of, checksum_file, glob_matches, parse_checksums, platform_of,
    selects,
};

/// A repository address, resolved to the API that serves its releases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    pub forge: GitForge,
    /// `owner/name` on GitHub, the full project path (groups included) on GitLab.
    pub path: String,
    /// The API root: `https://api.github.com`, `<host>/api/v3` for GitHub Enterprise Server,
    /// `<host>/api/v4` for GitLab.
    pub api: Url,
}

impl Repository {
    /// Reads a repository address as a person copies it from the browser.
    ///
    /// `https://github.com/owner/name`, with or without `.git`, a trailing slash or a page
    /// below it (`/releases`); for GitLab the project path is everything before `/-/`. A host
    /// other than `github.com` and `gitlab.com` needs `forge` named, because nothing in an
    /// address says which software serves it.
    pub fn parse(url: &Url, forge: Option<GitForge>) -> anyhow::Result<Self> {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("the repository address has no host"))?;
        let forge = forge
            .or_else(|| GitForge::of_host(host))
            .ok_or_else(|| anyhow::anyhow!("which forge serves {host} is not known"))?;
        let segments: Vec<&str> = url
            .path_segments()
            .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
            .unwrap_or_default();
        let segments: Vec<&str> = match forge {
            GitForge::Github => segments.into_iter().take(2).collect(),
            GitForge::Gitlab => segments
                .into_iter()
                .take_while(|segment| *segment != "-")
                .collect(),
        };
        if segments.len() < 2 {
            anyhow::bail!("the address does not name a repository (owner and name)");
        }
        let mut parts: Vec<String> = segments.iter().map(|part| (*part).to_owned()).collect();
        if let Some(last) = parts.last_mut()
            && let Some(stripped) = last.strip_suffix(".git")
        {
            *last = stripped.to_owned();
        }
        let path = parts.join("/");
        let api = match forge {
            GitForge::Github if GitForge::of_host(host) == Some(GitForge::Github) => {
                Url::parse("https://api.github.com")?
            }
            GitForge::Github => root(url)?.join("api/v3")?,
            GitForge::Gitlab => root(url)?.join("api/v4")?,
        };
        Ok(Self { forge, path, api })
    }

    /// The first page of the release list, newest first, `per_page` releases long.
    #[must_use]
    pub fn releases_url(&self, per_page: u32) -> Url {
        let mut url = self.api.clone();
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.pop_if_empty();
            match self.forge {
                GitForge::Github => {
                    segments.push("repos");
                    segments.extend(self.path.split('/'));
                }
                // One segment, `/` encoded: GitLab names a project by its URL-encoded path.
                GitForge::Gitlab => {
                    segments.push("projects");
                    segments.push(&self.path);
                }
            }
            segments.push("releases");
        }
        url.query_pairs_mut()
            .append_pair("per_page", &per_page.to_string());
        url
    }
}

/// `scheme://host[:port]/`, the root a self-hosted API lives under.
fn root(url: &Url) -> anyhow::Result<Url> {
    let mut root = url.clone();
    root.set_path("/");
    root.set_query(None);
    root.set_fragment(None);
    // A repository address carries no credentials worth keeping, and the API must not get any.
    let _ = root.set_username("");
    let _ = root.set_password(None);
    Ok(root)
}

/// One release, in the vocabulary both forges share.
#[derive(Clone, Debug)]
pub(crate) struct Release {
    /// GitHub's numeric id; GitLab's tag, which is all that names a GitLab release.
    pub id: String,
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub published_raw: Option<String>,
    pub assets: Vec<Asset>,
    /// The source archives the forge builds for every release.
    pub sources: Vec<Asset>,
}

/// One file of a release.
#[derive(Clone, Debug)]
pub(crate) struct Asset {
    /// The forge's id of the file, or the archive format of a source archive.
    pub id: String,
    pub name: String,
    /// Where it is downloaded from without a token.
    pub url: Url,
    /// GitHub's API address of the file, which a token can download from a private
    /// repository; `None` on GitLab and for source archives.
    pub api_url: Option<Url>,
    pub size: Option<u64>,
    /// Lowercase hex SHA-256 the forge states for the file (GitHub's `digest`).
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    id: u64,
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    created_at: Option<String>,
    tarball_url: Option<String>,
    zipball_url: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    id: u64,
    name: String,
    #[serde(default)]
    state: Option<String>,
    size: Option<u64>,
    browser_download_url: String,
    url: Option<String>,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GitlabRelease {
    tag_name: String,
    released_at: Option<String>,
    created_at: Option<String>,
    #[serde(default)]
    upcoming_release: bool,
    #[serde(default)]
    assets: GitlabAssets,
}

#[derive(Default, Deserialize)]
struct GitlabAssets {
    #[serde(default)]
    sources: Vec<GitlabSource>,
    #[serde(default)]
    links: Vec<GitlabLink>,
}

#[derive(Deserialize)]
struct GitlabSource {
    format: String,
    url: String,
}

#[derive(Deserialize)]
struct GitlabLink {
    id: u64,
    name: String,
    url: String,
    direct_asset_url: Option<String>,
}

/// Parses a release list in the forge's own format.
///
/// A file whose address does not parse is left out rather than failing the list; a list that
/// is not one fails the poll, because an empty answer would look like a repository without
/// releases.
pub(crate) fn parse_releases(
    forge: GitForge,
    body: &str,
    repository: &str,
) -> anyhow::Result<Vec<Release>> {
    match forge {
        GitForge::Github => {
            let releases: Vec<GithubRelease> = serde_json::from_str(body)
                .map_err(|error| anyhow::anyhow!("the release list is unreadable: {error}"))?;
            Ok(releases
                .into_iter()
                .map(|release| github_release(release, repository))
                .collect())
        }
        GitForge::Gitlab => {
            let releases: Vec<GitlabRelease> = serde_json::from_str(body)
                .map_err(|error| anyhow::anyhow!("the release list is unreadable: {error}"))?;
            Ok(releases.into_iter().map(gitlab_release).collect())
        }
    }
}

fn github_release(release: GithubRelease, repository: &str) -> Release {
    let published_raw = release.published_at.or(release.created_at);
    let name = repository.rsplit('/').next().unwrap_or(repository);
    let sources = [
        ("tar.gz", release.tarball_url),
        ("zip", release.zipball_url),
    ]
    .into_iter()
    .filter_map(|(format, address)| {
        let url = Url::parse(address.as_deref()?).ok()?;
        Some(Asset {
            id: format.to_owned(),
            name: format!("{name}-{}.{format}", release.tag_name),
            url,
            api_url: None,
            size: None,
            sha256: None,
        })
    })
    .collect();
    Release {
        id: release.id.to_string(),
        draft: release.draft,
        prerelease: release.prerelease,
        published_at: published_raw.as_deref().and_then(parse_instant),
        published_raw,
        assets: release
            .assets
            .into_iter()
            // A file still being uploaded is not a file yet; the next poll sees it finished.
            .filter(|asset| {
                asset
                    .state
                    .as_deref()
                    .is_none_or(|state| state == "uploaded")
            })
            .filter_map(|asset| {
                Some(Asset {
                    id: asset.id.to_string(),
                    url: Url::parse(&asset.browser_download_url).ok()?,
                    api_url: asset.url.as_deref().and_then(|url| Url::parse(url).ok()),
                    size: asset.size,
                    sha256: asset.digest.as_deref().and_then(sha256_of_digest),
                    name: asset.name,
                })
            })
            .collect(),
        sources,
        tag: release.tag_name,
    }
}

fn gitlab_release(release: GitlabRelease) -> Release {
    let published_raw = release.released_at.or(release.created_at);
    let project = |url: &str| {
        // `…/-/archive/v1.0/project-v1.0.zip`: the archive's own file name.
        url.rsplit('/').next().unwrap_or_default().to_owned()
    };
    Release {
        id: release.tag_name.clone(),
        draft: release.upcoming_release,
        prerelease: looks_like_prerelease(&release.tag_name),
        published_at: published_raw.as_deref().and_then(parse_instant),
        published_raw,
        assets: release
            .assets
            .links
            .into_iter()
            .filter_map(|link| {
                // The permanent `/-/releases/<tag>/downloads/…` address when there is one: it
                // survives the link's target moving.
                let address = link.direct_asset_url.as_deref().unwrap_or(&link.url);
                Some(Asset {
                    id: link.id.to_string(),
                    name: link.name,
                    url: Url::parse(address).ok()?,
                    api_url: None,
                    size: None,
                    sha256: None,
                })
            })
            .collect(),
        sources: release
            .assets
            .sources
            .into_iter()
            .filter_map(|source| {
                Some(Asset {
                    name: project(&source.url),
                    url: Url::parse(&source.url).ok()?,
                    id: source.format,
                    api_url: None,
                    size: None,
                    sha256: None,
                })
            })
            .collect(),
        tag: release.tag_name,
    }
}

fn parse_instant(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

/// GitHub's `sha256:<hex>`; any other algorithm is not one a download can be checked with here.
fn sha256_of_digest(digest: &str) -> Option<String> {
    digest
        .strip_prefix("sha256:")
        .filter(|hex| is_sha256(hex))
        .map(str::to_ascii_lowercase)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether a GitLab tag reads as a pre-release, which is the only place GitLab says so.
///
/// Semantic versioning's convention: a tag part after the version naming a pre-release stage.
#[must_use]
pub fn looks_like_prerelease(tag: &str) -> bool {
    const STAGES: &[&str] = &[
        "alpha", "beta", "rc", "pre", "preview", "dev", "nightly", "snapshot", "canary",
    ];
    tag.to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .skip(1)
        .any(|part| {
            STAGES.iter().any(|stage| {
                part.strip_prefix(stage)
                    .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))
            })
        })
}

#[cfg(test)]
#[path = "git_release_tests.rs"]
mod git_release_tests;
