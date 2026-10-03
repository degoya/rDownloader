//! What a git-release subscription asks a repository for (RD-190-13).
//!
//! A GitHub or GitLab repository publishes releases, and a release carries files — the
//! assets. A subscription to one downloads the assets of every new release once, and these
//! are the choices that decide which: the platform and architecture a file is built for, a
//! name pattern, whether pre-releases count, and whether the source archives every release
//! carries are wanted too. Drafts never are.
//!
//! Stored as one JSON object per subscription, like an indexer's search parameters, so an
//! option added later reads as its default in every row written before it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Most asset name patterns one subscription may carry.
pub const MAX_ASSET_PATTERNS: usize = 32;
/// Longest single asset name pattern, in characters.
pub const MAX_ASSET_PATTERN_CHARS: usize = 200;

/// Which API a repository speaks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitForge {
    /// GitHub, or a GitHub Enterprise Server at its own address.
    Github,
    /// GitLab, `gitlab.com` or a self-hosted instance.
    Gitlab,
}

impl GitForge {
    /// The forge a host is known to be, for the two public ones; `None` for any other host,
    /// whose forge the subscription has to name.
    #[must_use]
    pub fn of_host(host: &str) -> Option<Self> {
        match host.to_ascii_lowercase().as_str() {
            "github.com" | "www.github.com" => Some(Self::Github),
            "gitlab.com" | "www.gitlab.com" => Some(Self::Gitlab),
            _ => None,
        }
    }
}

/// An operating system a release file can be built for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitPlatform {
    Linux,
    Windows,
    Macos,
}

/// A processor architecture a release file can be built for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitArchitecture {
    /// 64-bit x86: `x86_64`, `amd64`, `x64`.
    X86_64,
    /// 64-bit ARM: `aarch64`, `arm64`.
    Aarch64,
    /// 32-bit x86: `i686`, `i386`, `x86`, `win32`.
    X86,
    /// 32-bit ARM: `armv7`, `armhf`, `arm`.
    Arm,
}

/// Which assets of a release a git-release subscription downloads.
///
/// Every list empty and both switches off downloads every asset of every full release —
/// which is what a subscription with no opinion should do.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct GitReleaseOptions {
    /// The API the repository speaks. `None` reads it from the host, which only works for
    /// `github.com` and `gitlab.com`; a self-hosted instance names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<GitForge>,
    /// Asset name patterns, `*` and `?` as wildcards, case-insensitive. When any are set, an
    /// asset must match one of them.
    pub asset_patterns: Vec<String>,
    /// When any are set, an asset must name one of these platforms — in its name or by a
    /// telling extension (`.exe`, `.dmg`, `.deb`, …). A file that names none is not taken, and
    /// neither is a checksum file.
    pub platforms: Vec<GitPlatform>,
    /// When any are set, an asset that names an architecture must name one of these. A file
    /// that names none — a universal macOS image, an installer — passes.
    pub architectures: Vec<GitArchitecture>,
    /// Whether pre-releases count as releases.
    pub prereleases: bool,
    /// Whether the source archives every release carries (`.zip`, `.tar.gz`) are downloaded.
    pub source_archives: bool,
}

impl GitReleaseOptions {
    /// Whether nothing is set, which is how every row written before the column reads.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{GitForge, GitReleaseOptions};

    #[test]
    fn an_empty_document_is_the_default_choice() {
        let read: GitReleaseOptions = serde_json::from_str("{}").expect("parse");
        assert!(read.is_empty());
        assert!(!read.prereleases);
    }

    #[test]
    fn only_the_two_public_hosts_name_their_forge() {
        assert_eq!(GitForge::of_host("GitHub.com"), Some(GitForge::Github));
        assert_eq!(GitForge::of_host("gitlab.com"), Some(GitForge::Gitlab));
        assert_eq!(GitForge::of_host("git.example.test"), None);
    }
}
