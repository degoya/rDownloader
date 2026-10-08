//! No plugin component carries the application's version.
//!
//! A crate's version goes into its component twice: into the `version` field of the component
//! metadata, and into the hash rustc mangles every symbol with. So a plugin crate on
//! `version.workspace = true` -- or one linking a workspace crate that is -- builds a different
//! component for every application release, although not a line of it changed. The version
//! guard of RD-120-47 then sees a changed component under an unchanged plugin version and
//! refuses to sign it, rightly: it cannot tell a version bump from a code change.
//!
//! It happened with release 1.2.3 on 2026-09-25: the bump from 1.2.2 changed all seventy
//! components linking the workspace version, and signing stopped. Releases 1.2.1 and 1.2.2 had
//! hidden it -- the first because the pipeline still ignored a refused plugin, the second
//! because every plugin was raised anyway for a contract change.
//!
//! Every plugin crate and every workspace crate a plugin links, directly or through another,
//! therefore carries its own `version`. Its value is inert; nothing reads it at run time.

use std::path::{Path, PathBuf};

use crate::workspace_root;

const INHERITED: &str = "version.workspace = true";

/// The directories named by `path = "…"` dependencies in a `Cargo.toml`, resolved against it;
/// those under `[dev-dependencies]` only with `dev`.
fn path_dependencies(manifest: &Path, dev: bool) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(manifest).unwrap_or_default();
    let directory = manifest.parent().expect("manifest directory");
    let mut in_dev_section = false;
    text.lines()
        .filter(|line| {
            if line.starts_with('[') {
                in_dev_section = line.contains("dev-dependencies");
            }
            dev || !in_dev_section
        })
        .filter_map(|line| line.split("path = \"").nth(1))
        .filter_map(|rest| rest.split('"').next())
        .filter(|path| !path.ends_with("wit"))
        .filter_map(|path| directory.join(path).canonicalize().ok())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect()
}

/// Every crate directory the plugin crates reach through path dependencies, the plugin crates
/// themselves included; through `[dev-dependencies]` too with `dev`.
fn reached_from_plugins(root: &Path, dev: bool) -> Vec<PathBuf> {
    let mut pending: Vec<PathBuf> = std::fs::read_dir(root.join("plugins"))
        .expect("plugins directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    assert!(!pending.is_empty(), "no plugin crates were found at all");

    let mut seen = Vec::new();
    while let Some(crate_directory) = pending.pop() {
        if seen.contains(&crate_directory) {
            continue;
        }
        pending.extend(path_dependencies(&crate_directory.join("Cargo.toml"), dev));
        seen.push(crate_directory);
    }
    assert!(
        seen.iter()
            .any(|path| path.ends_with("crates/rd-plugin-types")),
        "the walk no longer reaches rd-plugin-types, so it no longer follows path dependencies"
    );
    seen
}

/// The plugins link `rd-plugin-types`, never `rd-core` (RD-1190-08, CORE-06): a change to the
/// service's own types would otherwise build and test every plugin crate again, and send the
/// component check after every plugin. A plugin's tests may still name `rd-core` under
/// `[dev-dependencies]`; nothing of that reaches a component.
#[test]
fn no_plugin_links_rd_core() {
    let linked = reached_from_plugins(&workspace_root(), false);
    assert!(
        !linked.iter().any(|path| path.ends_with("crates/rd-core")),
        "a plugin crate links rd-core again, directly or through rd-plugin-api or a shared plugin \
         library; name the type from rd-plugin-types instead, or move it there if a plugin \
         really needs it"
    );
}

#[test]
fn no_plugin_crate_inherits_the_workspace_version() {
    let root = workspace_root();
    let seen = reached_from_plugins(&root, true);

    let mut offenders: Vec<String> = seen
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .is_ok_and(|text| text.lines().any(|line| line.trim() == INHERITED))
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these crates end up in a plugin component but take the application's version, so every \
         release changes every component and signing refuses them; give each its own \
         `version = \"…\"`:\n  {}",
        offenders.join("\n  ")
    );
}
