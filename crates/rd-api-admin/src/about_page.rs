//! The About page (RD-130-12): which build is running, where it comes from, who made it, and
//! which helper programs it runs under which licence.
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
//! * the helper programs are [`TOOLS`]. No release package contains one (RD-180-05); the container
//!   image does, and the page names where their licence texts lie in it (RD-1480-12). [`tests`]
//!   holds the table to `resources/vendor-licenses/`, which a local build copies beside the tools
//!   somebody put into `vendor/`.
//!
//! The dependency licences below the head are served by `rd-api` (its `about` module), which holds
//! `licenses/third-party.json` beside the assembled service (RD-160-06).

use axum::extract::State;
use rd_api_core::extract::Json;
use rd_update::InstallKind;
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
    /// The helper programs this installation runs: the ones the container image contains, or,
    /// everywhere else, the ones rDownloader fetches or finds on the machine.
    pub bundled_tools: Vec<BundledTool>,
    /// Whether this installation came with them. Only the container image does.
    pub tools_shipped: bool,
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

/// A helper program, with the licence it is under.
#[derive(Serialize, ToSchema)]
pub struct BundledTool {
    pub name: String,
    /// SPDX expression; `LicenseRef-…` where the licence has no SPDX identifier.
    pub license: String,
    /// Where the full text lies in this installation; empty where it came without the program.
    pub file: String,
    pub homepage: String,
}

/// A helper program rDownloader runs. 7-Zip's expression covers both its builds: the Linux `7zz`
/// carries the RAR code itself, the Windows `7z.exe` loads it from `7z.dll`.
struct Tool {
    name: &'static str,
    license: &'static str,
    /// Its text in the container image, where the image contains the program. Debian's packages
    /// bring theirs; `docker/python-licences.py` collects the Python ones.
    image_text: Option<&'static str>,
    /// Its licence in the image, where Debian builds it under another one.
    image_license: Option<&'static str>,
    homepage: &'static str,
}

/// The helper programs, by name. A text in `resources/vendor-licenses/` is named after its row
/// (`<name>-LICENSE.txt`); `tests::every_vendored_licence_text_has_its_tool` holds that.
const TOOLS: &[Tool] = &[
    Tool {
        name: "7-Zip",
        license: "LGPL-2.1-or-later AND BSD-3-Clause AND BSD-2-Clause AND LicenseRef-unRAR-restriction",
        image_text: Some("/usr/share/doc/7zip/copyright, /usr/share/doc/7zip-rar/copyright"),
        image_license: None,
        homepage: "https://www.7-zip.org",
    },
    Tool {
        name: "Apprise",
        license: "BSD-2-Clause",
        image_text: Some("/usr/share/doc/rdownloader/python/apprise/"),
        image_license: None,
        homepage: "https://github.com/caronc/apprise",
    },
    Tool {
        name: "FFmpeg",
        license: "GPL-3.0-or-later",
        image_text: Some("/usr/share/doc/ffmpeg/copyright"),
        image_license: Some("GPL-2.0-or-later"),
        homepage: "https://ffmpeg.org",
    },
    Tool {
        name: "gallery-dl",
        license: "GPL-2.0-only",
        image_text: Some("/usr/share/doc/rdownloader/python/gallery-dl/"),
        image_license: None,
        homepage: "https://github.com/mikf/gallery-dl",
    },
    Tool {
        name: "par2cmdline",
        license: "GPL-2.0-or-later",
        image_text: Some("/usr/share/doc/par2/copyright"),
        image_license: None,
        homepage: "https://github.com/Parchive/par2cmdline",
    },
    Tool {
        name: "rclone",
        license: "MIT",
        image_text: None,
        image_license: None,
        homepage: "https://rclone.org",
    },
    Tool {
        name: "Streamlink",
        license: "BSD-2-Clause",
        image_text: Some("/usr/share/doc/rdownloader/python/streamlink/"),
        image_license: None,
        homepage: "https://streamlink.github.io",
    },
    Tool {
        name: "UnRAR",
        license: "LicenseRef-UnRAR",
        image_text: None,
        image_license: None,
        homepage: "https://www.rarlab.com",
    },
    Tool {
        name: "yt-dlp",
        license: "Unlicense",
        image_text: Some("/usr/share/doc/rdownloader/python/yt-dlp/"),
        image_license: None,
        homepage: "https://github.com/yt-dlp/yt-dlp",
    },
];

/// The programs the page lists: the image's own with their texts, or all of them without.
fn tools(shipped: bool) -> Vec<BundledTool> {
    TOOLS
        .iter()
        .filter(|tool| !shipped || tool.image_text.is_some())
        .map(|tool| BundledTool {
            name: tool.name.to_owned(),
            license: tool
                .image_license
                .filter(|_| shipped)
                .unwrap_or(tool.license)
                .to_owned(),
            file: tool
                .image_text
                .filter(|_| shipped)
                .unwrap_or_default()
                .to_owned(),
            homepage: tool.homepage.to_owned(),
        })
        .collect()
}

fn about(build: &BuildInfo, kind: InstallKind) -> AboutResponse {
    let tools_shipped = kind == InstallKind::Docker;
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
        bundled_tools: tools(tools_shipped),
        tools_shipped,
    }
}

#[utoipa::path(get, path = "/api/v1/system/about", tag = "system", responses((status = 200, body = AboutResponse)))]
pub async fn system_about(State(state): State<AppState>) -> Json<AboutResponse> {
    Json(about(
        &state.runtime.build,
        state.admin.updates.install_kind(),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        path::{Path, PathBuf},
    };

    use rd_update::InstallKind;

    use super::{AboutLinkKind, BuildInfo, TOOLS, about};

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
    fn every_vendored_licence_text_has_its_tool() {
        let shared = licence_texts("resources/vendor-licenses");
        // A local build installs the shared texts plus its platform's, as the package scripts
        // do, and each must come out as one text per program.
        let [linux, windows] = ["linux", "windows"].map(|platform| {
            let own = licence_texts(&format!("resources/vendor-licenses/{platform}"));
            assert!(
                shared.is_disjoint(&own),
                "{platform}: a text is both shared and platform-specific: {:?}",
                shared.intersection(&own).collect::<Vec<_>>()
            );
            shared.union(&own).cloned().collect::<BTreeSet<String>>()
        });
        assert_eq!(linux, windows);
        for text in linux {
            let name = text
                .strip_suffix("-LICENSE.txt")
                .or_else(|| text.strip_suffix("-COPYING.txt"));
            assert!(
                TOOLS.iter().any(|tool| Some(tool.name) == name),
                "{text} belongs to no row of TOOLS"
            );
        }
        for tool in TOOLS {
            assert!(!tool.license.is_empty(), "{} has no licence", tool.name);
            assert!(
                tool.homepage.starts_with("https://"),
                "{}: {}",
                tool.name,
                tool.homepage
            );
        }
    }

    /// RD-1480-12: only the container image comes with helper programs. What an archive or an
    /// installer answers names no path of theirs, because neither contains one (RD-180-05).
    #[test]
    fn only_the_image_names_texts_of_tools_it_ships() {
        for kind in [InstallKind::Portable, InstallKind::Msi, InstallKind::Deb] {
            let answer = about(&BuildInfo::default(), kind);
            assert!(!answer.tools_shipped, "{kind:?}");
            assert_eq!(answer.bundled_tools.len(), TOOLS.len(), "{kind:?}");
            for tool in &answer.bundled_tools {
                assert_eq!(tool.file, "", "{kind:?}: {}", tool.name);
            }
        }

        let answer = about(&BuildInfo::default(), InstallKind::Docker);
        assert!(answer.tools_shipped);
        let names: Vec<&str> = answer
            .bundled_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "7-Zip",
                "Apprise",
                "FFmpeg",
                "gallery-dl",
                "par2cmdline",
                "Streamlink",
                "yt-dlp"
            ]
        );
        let dockerfile = read("docker/Dockerfile");
        for tool in &answer.bundled_tools {
            assert!(tool.file.starts_with("/usr/share/doc/"), "{}", tool.name);
        }
        // The packages the paths above belong to are the ones the image installs.
        for package in ["7zip", "7zip-rar", "ffmpeg", "par2"] {
            assert!(
                dockerfile
                    .lines()
                    .any(|line| line.trim() == format!("{package} \\")),
                "{package} is not installed"
            );
        }
        assert!(dockerfile.contains("/usr/share/doc/rdownloader/python"));
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
        let answer = about(&BuildInfo::default(), InstallKind::Portable);
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
        let answer = about(&BuildInfo::default(), InstallKind::Portable);
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
        let answer = about(&BuildInfo::new("unknown", " "), InstallKind::Portable);
        assert_eq!(answer.commit, None);
        assert_eq!(answer.built, None);
        let answer = about(
            &BuildInfo::new("1a2b3c4d-dirty", "2026-09-25T12:00:00Z"),
            InstallKind::Portable,
        );
        assert_eq!(answer.commit.as_deref(), Some("1a2b3c4d-dirty"));
        assert_eq!(answer.built.as_deref(), Some("2026-09-25T12:00:00Z"));
        assert_eq!(answer.authors, ["Alexander Herling / DEGOYA.digital"]);
        assert_eq!(answer.license, "GPL-3.0-or-later");
    }
}
