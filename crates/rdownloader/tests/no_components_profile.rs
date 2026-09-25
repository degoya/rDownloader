//! Every test file that loads a real plugin component is left out of the `no-components`
//! nextest profile.
//!
//! The `rust` CI job builds no components and runs with that profile; the `components` job
//! builds them and runs those files. A test file the profile does not name fails the `rust` job
//! on "has not been built in this checkout". Three such files reached GitHub on 2026-09-25 —
//! two of them load their components through `tests/support/`, which a search for the call in
//! the test file itself did not find.
//!
//! A test file counts as loading a component when it, or the `support` module it declares,
//! mentions `artifact::component`.

use std::path::{Path, PathBuf};

const LOADS_A_COMPONENT: &str = "artifact::component";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Every `.rs` file below `directory`, recursively.
fn sources(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found
}

/// The `binary(...)` entries of the profile, as exact names and as `/suffix$/` suffixes.
fn profile_entries(profile: &str) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut suffixes = Vec::new();
    for part in profile.split("binary(").skip(1) {
        let entry = part.split(')').next().unwrap_or_default().trim();
        if let Some(pattern) = entry.strip_prefix('/') {
            let suffix = pattern
                .strip_suffix("$/")
                .unwrap_or_else(|| panic!("the guard reads `binary(/suffix$/)` only, not {entry}"));
            assert!(
                suffix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "the guard reads a plain suffix only, not {entry}"
            );
            suffixes.push(suffix.to_owned());
        } else {
            names.push(entry.to_owned());
        }
    }
    (names, suffixes)
}

#[test]
fn every_test_file_that_loads_a_component_is_left_out_of_no_components() {
    let root = workspace_root();
    let profile = read(&root.join(".config/nextest.toml"));
    let section = profile
        .split("[profile.no-components]")
        .nth(1)
        .expect("the no-components profile");
    let (names, suffixes) = profile_entries(section);
    assert!(!names.is_empty(), "no binary(...) entries were read");

    let mut missing = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("crates directory")
        .flatten()
        .map(|entry| entry.path().join("tests"))
        .filter(|tests| tests.is_dir())
        .collect();
    crates.sort();
    for tests in crates {
        let support_loads = sources(&tests.join("support"))
            .iter()
            .any(|file| read(file).contains(LOADS_A_COMPONENT));
        let Ok(entries) = std::fs::read_dir(&tests) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.path();
            // This file names the call it looks for, and loads nothing.
            if file.extension().is_none_or(|extension| extension != "rs")
                || file
                    .file_name()
                    .is_some_and(|name| name == "no_components_profile.rs")
            {
                continue;
            }
            let text = read(&file);
            let loads = text.contains(LOADS_A_COMPONENT)
                || (support_loads && text.contains("mod support;"));
            if !loads {
                continue;
            }
            let binary = file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            let excluded = names.contains(&binary)
                || suffixes
                    .iter()
                    .any(|suffix| binary.ends_with(suffix.as_str()));
            if !excluded {
                missing.push(
                    file.strip_prefix(&root)
                        .unwrap_or(&file)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these test files load a plugin component but the `no-components` profile in \
         .config/nextest.toml does not leave them out, so the CI `rust` job fails on them; add \
         `binary(<file stem>)`:\n  {}",
        missing.join("\n  ")
    );
}
