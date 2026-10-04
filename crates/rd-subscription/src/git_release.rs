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

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rd_core::{GitArchitecture, GitForge, GitPlatform, GitReleaseOptions};
use serde::Deserialize;
use url::Url;

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
pub struct Release {
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
pub struct Asset {
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
pub fn parse_releases(
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

/// Whether the options select a file of this name.
#[must_use]
pub fn selects(options: &GitReleaseOptions, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !options.asset_patterns.is_empty()
        && !options
            .asset_patterns
            .iter()
            .any(|pattern| glob_matches(pattern, &lower))
    {
        return false;
    }
    // A checksum file is no build for any platform, whatever its name repeats of the file it
    // covers; it is read for the checksums either way.
    if !options.platforms.is_empty()
        && (checksum_file(name).is_some()
            || !platform_of(&lower).is_some_and(|platform| options.platforms.contains(&platform)))
    {
        return false;
    }
    if !options.architectures.is_empty()
        && let Some(architecture) = architecture_of(&lower)
        && !options.architectures.contains(&architecture)
    {
        return false;
    }
    true
}

/// Whether `pattern` (`*`, `?`) matches the whole of `name_lowercase`, ignoring case.
///
/// Matched directly rather than through a regular expression built and compiled for every
/// asset of every release (audit 1.9.1, INTAKE-10). The semantics are the ones that expression
/// had: `*` is any run of characters and `?` any one character, a line break excepted, and
/// everything else stands for itself.
#[must_use]
pub fn glob_matches(pattern: &str, name_lowercase: &str) -> bool {
    let pattern: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let name: Vec<char> = name_lowercase.chars().collect();
    let (mut at_pattern, mut at_name) = (0, 0);
    // The last `*` seen and the first name character it does not cover yet: where a failed
    // literal goes back to, letting that star take one character more.
    let mut star: Option<(usize, usize)> = None;
    while at_name < name.len() {
        let current = name[at_name];
        match pattern.get(at_pattern).copied() {
            Some('*') => {
                star = Some((at_pattern, at_name));
                at_pattern += 1;
            }
            Some('?') if current != '\n' => {
                at_pattern += 1;
                at_name += 1;
            }
            Some(literal) if literal != '*' && literal != '?' && literal == current => {
                at_pattern += 1;
                at_name += 1;
            }
            _ => match star {
                Some((star_at, covered)) if name[covered] != '\n' => {
                    star = Some((star_at, covered + 1));
                    at_pattern = star_at + 1;
                    at_name = covered + 1;
                }
                _ => return false,
            },
        }
    }
    pattern[at_pattern..]
        .iter()
        .all(|character| *character == '*')
}

/// Whether `token` stands in `name` as a word of its own — not inside a longer one. A token
/// written as an extension (`.exe`) has to end the name: `.pkg` is a macOS installer, while
/// `.pkg.tar.zst` is an Arch Linux package.
fn has_word(name: &str, token: &str) -> bool {
    if token.starts_with('.') {
        return name.ends_with(token);
    }
    name.match_indices(token).any(|(start, _)| {
        let before = name[..start].chars().next_back();
        let after = name[start + token.len()..].chars().next();
        !before.is_some_and(|character| character.is_ascii_alphanumeric())
            && !after.is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

/// The platform a file name names, by word or by a telling extension.
#[must_use]
pub fn platform_of(name_lowercase: &str) -> Option<GitPlatform> {
    const TABLE: &[(GitPlatform, &[&str])] = &[
        (
            GitPlatform::Windows,
            &[
                "windows", "win", "win32", "win64", "msvc", "mingw", ".exe", ".msi", ".msix",
            ],
        ),
        (
            GitPlatform::Macos,
            &["macos", "darwin", "osx", "mac", "apple", ".dmg", ".pkg"],
        ),
        (
            GitPlatform::Linux,
            &[
                "linux",
                "musl",
                ".appimage",
                ".deb",
                ".rpm",
                ".flatpak",
                ".snap",
                ".pkg.tar.zst",
                ".pkg.tar.xz",
            ],
        ),
    ];
    TABLE.iter().find_map(|(platform, tokens)| {
        tokens
            .iter()
            .any(|token| has_word(name_lowercase, token))
            .then_some(*platform)
    })
}

/// The architecture a file name names. The 64-bit spellings are tried first, because the
/// 32-bit ones are their prefixes (`x86` in `x86_64`, `arm` in `arm64`).
#[must_use]
pub fn architecture_of(name_lowercase: &str) -> Option<GitArchitecture> {
    const TABLE: &[(GitArchitecture, &[&str])] = &[
        (GitArchitecture::Aarch64, &["aarch64", "arm64", "armv8"]),
        (
            GitArchitecture::X86_64,
            &["x86_64", "x86-64", "amd64", "x64", "win64", "64bit"],
        ),
        (
            GitArchitecture::X86,
            &[
                "i386", "i486", "i586", "i686", "x86", "ia32", "win32", "32bit", "386",
            ],
        ),
        (
            GitArchitecture::Arm,
            &["armv7", "armv7l", "armhf", "armv6", "arm"],
        ),
    ];
    TABLE.iter().find_map(|(architecture, tokens)| {
        tokens
            .iter()
            .any(|token| has_word(name_lowercase, token))
            .then_some(*architecture)
    })
}

/// What a checksum file covers, by its name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChecksumFile {
    /// A list for every file of the release: `SHA256SUMS`, `checksums.txt`, `…_checksums.txt`.
    List,
    /// The checksum of one file, `<file>.sha256`.
    Single(String),
}

/// Whether a release file is a SHA-256 checksum file, and of what.
#[must_use]
pub fn checksum_file(name: &str) -> Option<ChecksumFile> {
    let lower = name.to_ascii_lowercase();
    for suffix in [".sha256", ".sha256sum"] {
        if lower.ends_with(suffix)
            && let Some(covered) = name.get(..name.len() - suffix.len())
            && !covered.is_empty()
        {
            return Some(ChecksumFile::Single(covered.to_owned()));
        }
    }
    let list = matches!(
        lower.as_str(),
        "sha256sums" | "sha256sums.txt" | "sha256sum.txt" | "checksums.txt" | "checksums.sha256"
    ) || lower.ends_with("_checksums.txt")
        || lower.ends_with("-checksums.txt")
        || lower.ends_with(".sha256sums");
    list.then_some(ChecksumFile::List)
}

/// Reads a SHA-256 checksum file: `<hex>  <name>` (GNU, `*` for binary mode), `SHA256 (<name>)
/// = <hex>` (BSD), or a lone `<hex>`, which is filed under the empty name.
///
/// Names are kept as their last path segment, since lists are often written from a build
/// directory (`./dist/tool.tar.gz`).
#[must_use]
pub fn parse_checksums(text: &str) -> BTreeMap<String, String> {
    let mut sums = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(rest) = line.strip_prefix("SHA256 (")
            && let Some((name, hex)) = rest.split_once(") = ")
            && is_sha256(hex.trim())
        {
            sums.insert(base_name(name), hex.trim().to_ascii_lowercase());
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hex) = parts.next().filter(|hex| is_sha256(hex)) else {
            continue;
        };
        let name = parts.next().map(str::trim).unwrap_or_default();
        let name = name.strip_prefix('*').unwrap_or(name);
        sums.insert(base_name(name), hex.to_ascii_lowercase());
    }
    sums
}

fn base_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_owned()
}

#[cfg(test)]
#[path = "git_release_tests.rs"]
mod git_release_tests;
