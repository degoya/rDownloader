//! The About page (RD-130-12): which build is running, where it comes from, who made it, and
//! which helper tools the packages ship under which licence.
//!
//! Behind the sign-in like the rest of `/api/v1/system`. `/api/v1/health` tells anybody the
//! version already, and that stays so; the commit and the build time say exactly which tree a
//! machine runs, and that is for the people who run it.
//!
//! Nothing here is kept twice:
//!
//! * the commit and the build time are the values VERSION.txt carries, compiled in by
//!   `crates/rdownloader/build.rs` and handed over as [`BuildInfo`];
//! * the version, the author, the licence and the repository come from `Cargo.toml`;
//! * the helper tools' licence texts are `resources/vendor-licenses/`, which the packaging copies
//!   into `vendor/licenses/` and [`tests`] holds to [`BUNDLED_TOOLS`].
//!
//! The dependency licences below the head are served by `rd-api` (its `about` module), which holds
//! `licenses/third-party.json` beside the assembled service (RD-160-06).

use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{AppState, BuildInfo};

/// Whether each address leads anywhere yet, decided per address and only once it does.
///
/// An address that is not reachable for a reader is named "not yet published" instead of drawn
/// as a link that ends in a 404 or a sign-in. The website and the public repository answer since
/// 2026-09-25 (checked: HTTP 200), and with the repository its changelog and its private
/// vulnerability reporting and its wiki, the handbook (first export with 1.3.0).
const SOURCE_PUBLISHED: bool = true;
const WEBSITE_PUBLISHED: bool = true;
const HANDBOOK_PUBLISHED: bool = true;
const SECURITY_PUBLISHED: bool = true;
const CHANGELOG_PUBLISHED: bool = true;

/// The website, which `Cargo.toml` has no field for.
const WEBSITE: &str = "https://rdownloader.net";

/// What the About page shows above the licence lists.
#[derive(Serialize, ToSchema)]
pub struct AboutResponse {
    pub version: String,
    /// Eight characters of the commit, `-dirty` when the tree differed from it, or the form
    /// a release build gives it. `null` when the binary was built without git.
    pub commit: Option<String>,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`. `null` when unknown.
    pub built: Option<String>,
    /// The `rdownloader:plugin` contract versions a plugin can be built against.
    pub plugin_contracts: Vec<String>,
    /// Operating system and architecture, in the words VERSION.txt uses (`linux x86_64`).
    pub platform: String,
    /// The licence of rDownloader itself, as an SPDX expression.
    pub license: String,
    pub authors: Vec<String>,
    pub links: Vec<AboutLink>,
    pub bundled_tools: Vec<BundledTool>,
}

/// One address the page offers.
#[derive(Serialize, ToSchema)]
pub struct AboutLink {
    pub kind: AboutLinkKind,
    /// `null` where the address is not decided yet (the handbook's was, until the release that
    /// publishes it): the interface then shows the mark alone rather than a guess.
    pub url: Option<String>,
    /// False until the first public release: the interface shows the address as text, marked
    /// "not yet published", rather than as a link.
    pub published: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AboutLinkKind {
    Source,
    Website,
    Handbook,
    /// Reporting a vulnerability privately.
    Security,
    /// The project's changelog, on the main branch.
    Changelog,
}

/// A helper program the packages ship in `vendor/`, with the licence it ships under.
#[derive(Serialize, ToSchema)]
pub struct BundledTool {
    pub name: String,
    /// SPDX expression; `LicenseRef-…` where the licence has no SPDX identifier.
    pub license: String,
    /// Where the full text lies in an installed package.
    pub file: String,
    pub homepage: String,
}

/// The helper tools of the Linux and Windows packages: name, licence, text file, homepage.
///
/// The files are `resources/vendor-licenses/<file>`, or `resources/vendor-licenses/<platform>/`
/// where the wording differs per build — 7-Zip's: the Linux `7zz` carries the RAR code itself,
/// the Windows `7z.exe` loads it from `7z.dll`, and each distribution words its License.txt
/// accordingly. Both come to the same expression. A tool added to the packages without its
/// text, or a text without its row, fails `tests::every_bundled_tool_ships_its_licence_text`.
const BUNDLED_TOOLS: &[(&str, &str, &str, &str)] = &[
    (
        "7-Zip",
        "LGPL-2.1-or-later AND BSD-3-Clause AND BSD-2-Clause AND LicenseRef-unRAR-restriction",
        "7-Zip-LICENSE.txt",
        "https://www.7-zip.org",
    ),
    (
        "FFmpeg",
        "GPL-3.0-or-later",
        "FFmpeg-LICENSE.txt",
        "https://ffmpeg.org",
    ),
    (
        "gallery-dl",
        "GPL-2.0-only",
        "gallery-dl-LICENSE.txt",
        "https://github.com/mikf/gallery-dl",
    ),
    ("rclone", "MIT", "rclone-COPYING.txt", "https://rclone.org"),
    (
        "Streamlink",
        "BSD-2-Clause",
        "Streamlink-LICENSE.txt",
        "https://streamlink.github.io",
    ),
    (
        "UnRAR",
        "LicenseRef-UnRAR",
        "UnRAR-LICENSE.txt",
        "https://www.rarlab.com",
    ),
    (
        "yt-dlp",
        "Unlicense",
        "yt-dlp-LICENSE.txt",
        "https://github.com/yt-dlp/yt-dlp",
    ),
];

fn about(build: &BuildInfo) -> AboutResponse {
    let version = env!("CARGO_PKG_VERSION");
    let repository = env!("CARGO_PKG_REPOSITORY");
    let link = |kind, url: Option<String>, published: bool| AboutLink {
        kind,
        // Published without an address is not a state a reader can follow.
        published: published && url.is_some(),
        url,
    };
    AboutResponse {
        version: version.to_owned(),
        commit: build.commit().map(str::to_owned),
        built: build.built().map(str::to_owned),
        plugin_contracts: rd_plugin_host::SUPPORTED_API_VERSIONS
            .iter()
            .map(|api| format!("rdownloader:plugin@{api}"))
            .collect(),
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        license: env!("CARGO_PKG_LICENSE").to_owned(),
        authors: env!("CARGO_PKG_AUTHORS")
            .split(':')
            .filter(|author| !author.is_empty())
            .map(str::to_owned)
            .collect(),
        links: vec![
            link(
                AboutLinkKind::Source,
                Some(repository.to_owned()),
                SOURCE_PUBLISHED,
            ),
            link(
                AboutLinkKind::Website,
                Some(WEBSITE.to_owned()),
                WEBSITE_PUBLISHED,
            ),
            link(
                AboutLinkKind::Handbook,
                Some(format!("{repository}/wiki")),
                HANDBOOK_PUBLISHED,
            ),
            link(
                AboutLinkKind::Security,
                Some(format!("{repository}/security/advisories/new")),
                SECURITY_PUBLISHED,
            ),
            link(
                AboutLinkKind::Changelog,
                Some(format!("{repository}/blob/main/CHANGELOG.md")),
                CHANGELOG_PUBLISHED,
            ),
        ],
        bundled_tools: BUNDLED_TOOLS
            .iter()
            .map(|(name, license, file, homepage)| BundledTool {
                name: (*name).to_owned(),
                license: (*license).to_owned(),
                file: format!("vendor/licenses/{file}"),
                homepage: (*homepage).to_owned(),
            })
            .collect(),
    }
}

#[utoipa::path(get, path = "/api/v1/system/about", tag = "system", responses((status = 200, body = AboutResponse)))]
pub async fn system_about(State(state): State<AppState>) -> Json<AboutResponse> {
    Json(about(&state.build))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        path::{Path, PathBuf},
    };

    use super::{AboutLinkKind, BUNDLED_TOOLS, BuildInfo, about};

    /// Read at run time, not with `env!`: a test binary can outlive the checkout that built it
    /// when worktrees share one target directory.
    fn workspace_file(relative: &str) -> PathBuf {
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir");
        Path::new(&manifest).join("../..").join(relative)
    }

    fn read(relative: &str) -> String {
        std::fs::read_to_string(workspace_file(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    fn licence_texts(relative: &str) -> BTreeSet<String> {
        let directory = workspace_file(relative);
        std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
            .map(|entry| entry.expect("a directory entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".txt"))
            .collect()
    }

    #[test]
    fn every_bundled_tool_ships_its_licence_text() {
        let named: BTreeSet<String> = BUNDLED_TOOLS
            .iter()
            .map(|(_, _, file, _)| (*file).to_owned())
            .collect();
        let shared = licence_texts("resources/vendor-licenses");
        // Each package installs the shared texts plus its platform's, as the package scripts
        // do, and each must come out as exactly one text per tool.
        for platform in ["linux", "windows"] {
            let own = licence_texts(&format!("resources/vendor-licenses/{platform}"));
            assert!(
                shared.is_disjoint(&own),
                "{platform}: a text is both shared and platform-specific: {:?}",
                shared.intersection(&own).collect::<Vec<_>>()
            );
            let package: BTreeSet<String> = shared.union(&own).cloned().collect();
            assert_eq!(
                package, named,
                "the {platform} package's texts and BUNDLED_TOOLS disagree"
            );
        }
        for (name, license, _, homepage) in BUNDLED_TOOLS {
            assert!(!license.is_empty(), "{name} has no licence");
            assert!(homepage.starts_with("https://"), "{name}: {homepage}");
        }
    }

    /// The contract the page names is the one the WIT file declares, not a copy of it.
    #[test]
    fn the_plugin_contract_is_the_wit_package() {
        let wit = read("crates/rd-plugin-api/wit/rdownloader.wit");
        let package = wit
            .lines()
            .find_map(|line| line.trim().strip_prefix("package "))
            .and_then(|rest| rest.strip_suffix(';'))
            .expect("the WIT file declares its package");
        let answer = about(&BuildInfo::default());
        assert!(
            answer
                .plugin_contracts
                .iter()
                .any(|contract| contract == package),
            "{package} is not among {:?}",
            answer.plugin_contracts
        );
    }

    #[test]
    fn only_a_published_address_is_offered_as_a_link() {
        let answer = about(&BuildInfo::default());
        let kinds: Vec<_> = answer.links.iter().map(|link| link.kind).collect();
        assert_eq!(
            kinds,
            [
                AboutLinkKind::Source,
                AboutLinkKind::Website,
                AboutLinkKind::Handbook,
                AboutLinkKind::Security,
                AboutLinkKind::Changelog,
            ]
        );
        let published: Vec<bool> = answer.links.iter().map(|link| link.published).collect();
        // Every address is public since the first export with 1.3.0.
        assert_eq!(published, [true, true, true, true, true]);
        for link in &answer.links {
            if let Some(url) = &link.url {
                assert!(url.starts_with("https://"), "{url}");
            }
            // An address is published only once it exists.
            assert!(!link.published || link.url.is_some(), "{:?}", link.kind);
        }
        assert_eq!(
            answer.links[2].url.as_deref(),
            Some("https://github.com/degoya/rDownloader/wiki")
        );
        assert_eq!(
            answer.links[4].url.as_deref(),
            Some("https://github.com/degoya/rDownloader/blob/main/CHANGELOG.md")
        );
    }

    #[test]
    fn unknown_is_no_value() {
        let answer = about(&BuildInfo::new("unknown", " "));
        assert_eq!(answer.commit, None);
        assert_eq!(answer.built, None);
        let answer = about(&BuildInfo::new("1a2b3c4d-dirty", "2026-09-25T12:00:00Z"));
        assert_eq!(answer.commit.as_deref(), Some("1a2b3c4d-dirty"));
        assert_eq!(answer.built.as_deref(), Some("2026-09-25T12:00:00Z"));
        assert_eq!(answer.authors, ["Alexander Herling"]);
        assert_eq!(answer.license, "GPL-3.0-or-later");
    }
}
