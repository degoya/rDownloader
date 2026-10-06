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

use crate::workspace_root;

const LOADS_A_COMPONENT: &str = "artifact::component";

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

/// The text of profile `name` in `.config/nextest.toml`, up to the next profile.
fn profile_section<'a>(profile: &'a str, name: &str) -> &'a str {
    let section = profile
        .split(&format!("[profile.{name}]"))
        .nth(1)
        .unwrap_or_else(|| panic!("the {name} profile"));
    section.split("\n[profile.").next().unwrap_or_default()
}

/// A profile's `default-filter`, its whitespace collapsed.
fn default_filter(section: &str) -> String {
    let filter = section
        .split("default-filter = \"\"\"")
        .nth(1)
        .and_then(|rest| rest.split("\"\"\"").next())
        .expect("a default-filter");
    filter.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The test binaries below a crate's `tests/`: a file is one, and so is a directory with a
/// `main.rs`, each of its modules part of it (RD-150-10, RD-1120-08). Each with its name and
/// the files it is built from.
fn test_binaries(tests: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let Ok(entries) = std::fs::read_dir(tests) else {
        return Vec::new();
    };
    let mut binaries = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        if path.is_dir() && path.join("main.rs").is_file() {
            binaries.push((name, sources(&path)));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            binaries.push((name, vec![path]));
        }
    }
    binaries
}

#[test]
fn every_test_file_that_loads_a_component_is_left_out_of_no_components() {
    let root = workspace_root();
    let profile = read(&root.join(".config/nextest.toml"));
    let (names, suffixes) = profile_entries(profile_section(&profile, "no-components"));
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
        for (binary, files) in test_binaries(&tests) {
            let excluded = names.contains(&binary)
                || suffixes
                    .iter()
                    .any(|suffix| binary.ends_with(suffix.as_str()));
            for file in files {
                // This file names the call it looks for, and loads nothing.
                if file
                    .file_name()
                    .is_some_and(|name| name == "no_components_profile.rs")
                {
                    continue;
                }
                let text = read(&file);
                let loads = text.contains(LOADS_A_COMPONENT)
                    || (support_loads && text.contains("mod support;"));
                // A suite inside a shared binary is left out by its tests' names instead.
                if loads && !excluded && !text.contains("on_real_components") {
                    missing.push(
                        file.strip_prefix(&root)
                            .unwrap_or(&file)
                            .display()
                            .to_string(),
                    );
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these test files load a plugin component but the `no-components` profile in \
         .config/nextest.toml does not leave them out, so the CI `rust` job fails on them; add \
         `binary(<binary name>)`:\n  {}",
        missing.join("\n  ")
    );
}

/// The `components` profile runs exactly what `no-components` leaves out (RD-1120-08): the CI
/// `components` job runs it, and a test left out of one and not named in the other would run
/// nowhere.
#[test]
fn the_components_profile_selects_what_no_components_leaves_out() {
    let profile = read(&workspace_root().join(".config/nextest.toml"));
    let left_out = default_filter(profile_section(&profile, "no-components"));
    let selected = default_filter(profile_section(&profile, "components"));
    let inner = left_out
        .strip_prefix("not (")
        .and_then(|rest| rest.strip_suffix(')'))
        .map(str::trim)
        .expect("no-components reads `not ( ... )`");
    assert_eq!(selected, inner);
}
