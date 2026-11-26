//! The About page's licence list (RD-130-12): the dependency licences every shipped artefact
//! contains, served as `scripts/licenses.sh` wrote them.
//!
//! The list is `licenses/third-party.json`, written by `scripts/licenses.sh` from
//! `cargo metadata` and `web/pnpm-lock.yaml` and held to both lockfiles by [`tests`]; the licence
//! and copyright texts in full are `licenses/THIRD-PARTY-NOTICES.txt`, which the same script writes
//! and every package carries as a file too (RD-1480-12). The page's head — version, build, links and the bundled tools — is `rd_api_admin::about_page`, where the
//! MCP toolbox reaches it too (RD-160-06).

use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The dependency list `scripts/licenses.sh` generates. Served as it is: parsing it on every
/// request would only prove again what `tests` proves once.
const THIRD_PARTY: &str = include_str!("../licenses/third-party.json");

/// Their licence, copyright and NOTICE texts, the same file every package carries.
const NOTICES: &str = include_str!("../licenses/THIRD-PARTY-NOTICES.txt");

/// The dependency licences, as `scripts/licenses.sh` writes them.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThirdPartyLicenses {
    /// Every crate a shipped artefact contains: what the binaries link on Linux, Windows and
    /// macOS, and what the plugin components link.
    pub rust: Vec<ThirdPartyPackage>,
    /// The other crates of `Cargo.lock`, as `name@version`: proc macros, build scripts, tests,
    /// and dependencies no shipped target or feature set reaches. They ship nowhere; they are
    /// listed so that a crate new to the lockfile can be told apart from one this list forgot.
    pub rust_not_shipped: Vec<String>,
    /// Every package of `web/pnpm-lock.yaml` the production dependencies reach.
    pub npm: Vec<ThirdPartyPackage>,
    /// What ships and is neither: the Rust standard library and runtime.
    pub other: Vec<ThirdPartyPackage>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThirdPartyPackage {
    pub name: String,
    pub version: String,
    /// SPDX expression as the package declares it, or as `licenses/overrides.json` records it
    /// for a package that declares none.
    pub license: String,
    /// Where the source code of exactly this version is offered.
    pub source: String,
}

/// A separate route from the page's head: the list runs to about a thousand entries, and the
/// MCP tool that reads the head has no use for it.
#[utoipa::path(get, path = "/api/v1/system/about/licenses", tag = "system", responses((status = 200, body = ThirdPartyLicenses)))]
pub(crate) async fn system_about_licenses() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], THIRD_PARTY).into_response()
}

/// The licence and copyright texts of everything a package ships, in full. A route of its own:
/// the file is close to two megabytes.
#[utoipa::path(get, path = "/api/v1/system/about/notices", tag = "system", responses((status = 200, description = "The licence and copyright texts of every component a package ships", content_type = "text/plain", body = String)))]
pub(crate) async fn system_about_notices() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        NOTICES,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::{Path, PathBuf},
    };

    use super::{NOTICES, THIRD_PARTY};

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
        for package in list.rust.iter().chain(&list.npm).chain(&list.other) {
            assert!(
                !package.license.trim().is_empty(),
                "{}@{} has no licence; record it in crates/rd-api/licenses/overrides.json with \
                 its source, then {REGENERATE}",
                package.name,
                package.version
            );
            assert!(
                package.source.starts_with("https://"),
                "{}@{} names no source code; {REGENERATE}",
                package.name,
                package.version
            );
        }
    }

    /// Every listed component has its entry in the texts, with its licence and its source, and
    /// every text an entry names is there. A toolchain update changes the standard library both
    /// name, so it fails here until the list follows.
    #[test]
    fn the_notices_cover_every_listed_component() {
        let list = listed();
        let lines: BTreeSet<&str> = NOTICES.lines().collect();
        let mut absent = Vec::new();
        for package in list.rust.iter().chain(&list.npm).chain(&list.other) {
            let entry = format!("{} {} ({})", package.name, package.version, package.license);
            let source = format!("    source: {}", package.source);
            if !lines.contains(entry.as_str()) || !lines.contains(source.as_str()) {
                absent.push(entry);
            }
        }
        assert!(
            absent.is_empty(),
            "not in the notices: {absent:?}; {REGENERATE}"
        );
        let marks = |prefix: &str| -> BTreeSet<&'static str> {
            NOTICES
                .lines()
                .filter_map(|line| line.strip_prefix(prefix)?.split_once(']'))
                .map(|(mark, _)| mark)
                .collect()
        };
        let referenced = marks("    text:   [");
        let printed = marks("[");
        assert!(!referenced.is_empty(), "the notices name no text at all");
        let unprinted: Vec<_> = referenced.difference(&printed).collect();
        assert!(
            unprinted.is_empty(),
            "named but not printed: {unprinted:?}; {REGENERATE}"
        );

        let toolchain = read("rust-toolchain.toml");
        let channel = toolchain
            .lines()
            .find_map(|line| line.strip_prefix("channel = \"")?.strip_suffix('"'))
            .expect("rust-toolchain.toml names a channel");
        assert!(
            list.other.iter().any(|package| package.version == channel),
            "the list names no standard library {channel}; {REGENERATE}"
        );
    }

    /// What the licences ask for word by word: the MIT and BSD conditions, Apache-2.0 in full with
    /// the NOTICE files of section 4(d), the fonts' and the icons' licence.
    #[test]
    fn the_notices_carry_the_texts_the_licences_ask_for() {
        for text in [
            "Permission is hereby granted, free of charge",
            "Redistributions in binary form must reproduce the above copyright",
            "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
            "Apache Arrow Object Store",
            "Mozilla Public License Version 2.0",
            "UNICODE LICENSE V3",
            "SIL OPEN FONT LICENSE Version 1.1",
            "Lucide Icons and Contributors",
        ] {
            assert!(NOTICES.contains(text), "{text:?}; {REGENERATE}");
        }
    }
}
