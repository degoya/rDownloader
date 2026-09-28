//! The About page (RD-130-12): which build is running, where it comes from, who made it, and
//! under which licences the parts it ships stand.
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
//! * the dependency licences are `licenses/third-party.json`, written by `scripts/licenses.sh`
//!   from `cargo metadata` and `web/pnpm-lock.yaml` and held to both lockfiles by [`tests`];
//! * the helper tools' licence texts are `resources/vendor-licenses/`, which the packaging copies
//!   into `vendor/licenses/` and [`tests`] holds to [`BUNDLED_TOOLS`].

use axum::{
    Json,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;

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

/// The dependency list `scripts/licenses.sh` generates. Served as it is: parsing it on every
/// request would only prove again what `tests` proves once.
const THIRD_PARTY: &str = include_str!("../licenses/third-party.json");

/// Commit and build time of the running binary.
///
/// Handed in by the binary rather than read here, because the build script that knows them
/// belongs to the binary crate. A test router never sets them and answers with neither.
#[derive(Clone, Debug, Default)]
pub struct BuildInfo {
    commit: Option<String>,
    built: Option<String>,
}

impl BuildInfo {
    /// `unknown` is what VERSION.txt and the build script write without git; it is no value.
    #[must_use]
    pub fn new(commit: &str, built: &str) -> Self {
        let known = |value: &str| {
            let value = value.trim();
            (!value.is_empty() && value != "unknown").then(|| value.to_owned())
        };
        Self {
            commit: known(commit),
            built: known(built),
        }
    }
}

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

/// The dependency licences, as `scripts/licenses.sh` writes them.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThirdPartyLicenses {
    /// Every crate a shipped artefact contains: what the binaries link on Linux, Windows and
    /// macOS, and what the plugin components link.
    pub rust: Vec<ThirdPartyPackage>,
    /// The other crates of `Cargo.lock`, as `name@version`: proc macros, build scripts, tests,
    /// and dependencies no shipped target or feature set reaches. They ship nowhere; they are
    /// listed so that a crate new to the lockfile can be told apart from one this list forgot.
    pub rust_not_shipped: Vec<String>,
    /// Every package of `web/pnpm-lock.yaml` the production dependencies reach.
    pub npm: Vec<ThirdPartyPackage>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThirdPartyPackage {
    pub name: String,
    pub version: String,
    /// SPDX expression as the package declares it, or as `licenses/overrides.json` records it
    /// for a package that declares none.
    pub license: String,
}

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
        commit: build.commit.clone(),
        built: build.built.clone(),
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

/// A separate route from the page's head: the list runs to about a thousand entries, and the
/// MCP tool that reads the head has no use for it.
#[utoipa::path(get, path = "/api/v1/system/about/licenses", tag = "system", responses((status = 200, body = ThirdPartyLicenses)))]
pub async fn system_about_licenses() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], THIRD_PARTY).into_response()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::{Path, PathBuf},
    };

    use super::{AboutLinkKind, BUNDLED_TOOLS, BuildInfo, THIRD_PARTY, about};

    const REGENERATE: &str = "run scripts/licenses.sh";

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

    fn listed() -> super::ThirdPartyLicenses {
        serde_json::from_str(THIRD_PARTY)
            .unwrap_or_else(|error| panic!("licenses/third-party.json: {error}; {REGENERATE}"))
    }

    /// `name@version` of every package in `Cargo.lock` that comes from somewhere — a registry
    /// or git. Workspace members carry no `source` line and are rDownloader itself.
    fn locked_crates() -> BTreeSet<String> {
        let mut crates = BTreeSet::new();
        for block in read("Cargo.lock").split("[[package]]").skip(1) {
            let field = |key: &str| {
                block.lines().find_map(|line| {
                    line.strip_prefix(key)
                        .and_then(|rest| rest.strip_prefix(" = \""))
                        .and_then(|rest| rest.strip_suffix('"'))
                })
            };
            if field("source").is_some() {
                let name = field("name").expect("a locked package has a name");
                let version = field("version").expect("a locked package has a version");
                crates.insert(format!("{name}@{version}"));
            }
        }
        crates
    }

    /// `name@version` of every package the production dependencies reach in
    /// `web/pnpm-lock.yaml`: the same walk as the generator, so a disagreement is a stale list
    /// and not two opinions. It starts at the importer's `dependencies` and
    /// `optionalDependencies` and follows each snapshot's own, except a snapshot's optional
    /// peers, which resolve only to what something else installs.
    fn locked_npm_packages() -> BTreeSet<String> {
        fn unquote(text: &str) -> &str {
            text.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
                .unwrap_or(text)
        }
        fn without_peers(key: &str) -> &str {
            key.split_once('(').map_or(key, |(base, _)| base)
        }
        // The snapshot a dependency points at: `name@version(peers)`, or for an alias the
        // `real-name@version` its value names. A `link:` is a local directory, no package.
        fn snapshot(name: &str, value: &str) -> Option<String> {
            if value.starts_with("link:") {
                None
            } else if value.starts_with(|c: char| c.is_ascii_digit()) {
                Some(format!("{name}@{value}"))
            } else {
                Some(value.to_owned())
            }
        }

        // Two YAML documents, pnpm's own install first and the project second; only the
        // project's importer has `dependencies`, so a line walk needs no telling them apart.
        let lockfile = read("web/pnpm-lock.yaml");
        let mut queue = Vec::new();
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut optional_peers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let (mut section, mut group) = ("", "");
        let (mut entry, mut dependency) = (String::new(), String::new());
        for line in lockfile.lines() {
            let text = line.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            let indent = line.len() - line.trim_start_matches(' ').len();
            let bare = text.strip_suffix(':').unwrap_or(text);
            let dependencies = matches!(group, "dependencies" | "optionalDependencies");
            if indent == 0 {
                section = bare;
            } else if indent == 2 {
                let key = text.strip_suffix(" {}").unwrap_or(text);
                entry = unquote(key.strip_suffix(':').unwrap_or(key)).to_owned();
                if section == "snapshots" {
                    edges.insert(entry.clone(), Vec::new());
                }
            } else if indent == 4 {
                group = bare;
            } else if section == "importers" && dependencies {
                if indent == 6 {
                    dependency = unquote(bare).to_owned();
                } else if indent == 8
                    && let Some(version) = text.strip_prefix("version: ")
                {
                    queue.extend(snapshot(&dependency, version));
                }
            } else if section == "packages" && group == "peerDependenciesMeta" {
                if indent == 6 {
                    dependency = unquote(bare).to_owned();
                } else if indent == 8 && text == "optional: true" {
                    optional_peers
                        .entry(entry.clone())
                        .or_default()
                        .insert(dependency.clone());
                }
            } else if section == "snapshots" && dependencies && indent == 6 {
                let (name, value) = text.split_once(": ").unwrap_or((text, ""));
                let name = unquote(name);
                let optional_peer = optional_peers
                    .get(without_peers(&entry))
                    .is_some_and(|peers| peers.contains(name));
                if !optional_peer && let Some(key) = snapshot(name, value) {
                    edges
                        .get_mut(&entry)
                        .expect("a snapshot's dependencies follow its key")
                        .push(key);
                }
            }
        }

        let mut reached = BTreeSet::new();
        while let Some(key) = queue.pop() {
            let next = edges
                .get(&key)
                .unwrap_or_else(|| panic!("pnpm-lock.yaml has no snapshot {key}"));
            if reached.insert(key) {
                queue.extend(next.iter().cloned());
            }
        }
        reached
            .iter()
            .map(|key| without_peers(key).to_owned())
            .collect()
    }

    fn difference(expected: &BTreeSet<String>, actual: &BTreeSet<String>) -> String {
        let missing: Vec<_> = expected.difference(actual).collect();
        let stale: Vec<_> = actual.difference(expected).collect();
        format!("not listed: {missing:?}; listed but no longer locked: {stale:?}; {REGENERATE}")
    }

    #[test]
    fn every_locked_crate_is_listed_or_known_not_to_ship() {
        let list = listed();
        let mut accounted: BTreeSet<String> = list
            .rust
            .iter()
            .map(|package| format!("{}@{}", package.name, package.version))
            .collect();
        accounted.extend(list.rust_not_shipped.iter().cloned());
        let locked = locked_crates();
        assert!(!locked.is_empty(), "Cargo.lock names no crate at all");
        assert_eq!(locked, accounted, "{}", difference(&locked, &accounted));
    }

    #[test]
    fn every_production_npm_package_is_listed() {
        let listed: BTreeSet<String> = listed()
            .npm
            .iter()
            .map(|package| format!("{}@{}", package.name, package.version))
            .collect();
        let locked = locked_npm_packages();
        assert!(!locked.is_empty(), "pnpm-lock.yaml names no package");
        assert_eq!(locked, listed, "{}", difference(&locked, &listed));
    }

    /// The acceptance criterion itself: a dependency without a licence entry fails the build.
    #[test]
    fn every_listed_dependency_names_its_licence() {
        let list = listed();
        for package in list.rust.iter().chain(&list.npm) {
            assert!(
                !package.license.trim().is_empty(),
                "{}@{} has no licence; record it in crates/rd-api/licenses/overrides.json with \
                 its source, then {REGENERATE}",
                package.name,
                package.version
            );
        }
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
