//! Every test that reads the version is in the release chain's version-dependent set (RD-1300-06).
//!
//! The release chain's test step reuses a `--full` green of the tree before the version bump when
//! the bump changed version lines only (RD-140-06). That green never ran a test reading the
//! version with the new one: 1.28.0 shipped with `the_baselines_are_the_last_two_releases` red on
//! `main`. So the step runs the tests of `scripts/lib/version-tests.list` after the bump, and this
//! lint keeps that list complete: a test file under `crates/` that reads `CARGO_PKG_VERSION` or
//! `RD_RELEASE_VERSION` in test code and is not listed fails here, and so does a listed file that
//! no longer reads either.
//!
//! Test code is a file under a `tests/` directory, a file named `tests.rs` or `*_tests.rs` (what a
//! `#[cfg(test)] #[path = "…"] mod tests;` includes), or an inline module opened right after a
//! `#[cfg(test)]` at column 0 and closed by the next `}` at column 0. Comments do not count.

use std::path::{Path, PathBuf};

use crate::workspace_root;

/// Built from halves, so that this file, test code itself, does not read as reading the version.
const NEEDLES: [&str; 2] = [
    concat!("CARGO_PKG", "_VERSION"),
    concat!("RD_RELEASE", "_VERSION"),
];
const LIST: &str = "scripts/lib/version-tests.list";

fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// `relative` with `/` between its parts on every host, as the list writes it.
fn slashed(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn whole_file_is_test_code(relative: &str) -> bool {
    let path = Path::new(relative);
    let under_tests = path
        .parent()
        .is_some_and(|parent| parent.components().any(|part| part.as_os_str() == "tests"));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    under_tests || stem == "tests" || stem.ends_with("_tests")
}

fn opens_inline_module(line: &str) -> bool {
    let item = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    item.starts_with("mod ") && line.ends_with('{')
}

/// The lines of `text` that are test code, comments left out.
fn test_lines<'a>(relative: &str, text: &'a str) -> Vec<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut code = Vec::new();
    if whole_file_is_test_code(relative) {
        code = lines;
    } else {
        let mut index = 0;
        while index < lines.len() {
            if lines[index] == "#[cfg(test)]" {
                let mut next = index + 1;
                while lines.get(next).is_some_and(|line| line.starts_with("#[")) {
                    next += 1;
                }
                if lines
                    .get(next)
                    .is_some_and(|line| opens_inline_module(line))
                {
                    let mut end = next + 1;
                    while lines.get(end).is_some_and(|line| *line != "}") {
                        code.push(lines[end]);
                        end += 1;
                    }
                    index = end;
                }
            }
            index += 1;
        }
    }
    code.retain(|line| !line.trim_start().starts_with("//"));
    code
}

fn reads_the_version(relative: &str, text: &str) -> bool {
    test_lines(relative, text)
        .iter()
        .any(|line| NEEDLES.iter().any(|needle| line.contains(needle)))
}

/// The files the list names: the first word of every line that is not a comment.
fn listed(list: &str) -> Vec<String> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// What `found` names and `listed` does not.
fn unlisted(found: &[String], listed: &[String]) -> Vec<String> {
    found
        .iter()
        .filter(|file| !listed.contains(file))
        .cloned()
        .collect()
}

fn version_reading_test_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(
        !files.is_empty(),
        "no Rust sources were found under crates/"
    );
    let mut found: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let relative = slashed(path.strip_prefix(root).unwrap_or(path));
            let text = std::fs::read_to_string(path).ok()?;
            reads_the_version(&relative, &text).then_some(relative)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn every_test_reading_the_version_is_in_the_release_set() {
    let root = workspace_root();
    let list = std::fs::read_to_string(root.join(LIST)).expect("scripts/lib/version-tests.list");
    let listed = listed(&list);
    let found = version_reading_test_files(&root);

    let missing = unlisted(&found, &listed);
    assert!(
        missing.is_empty(),
        "these test files read the version, but {LIST} does not name them, so the release chain \
         never runs them with the new version when it reuses the green before the bump; add a \
         line for each (package, target, nextest filter):\n  {}",
        missing.join("\n  ")
    );
    let stale = unlisted(&listed, &found);
    assert!(
        stale.is_empty(),
        "{LIST} names files that no longer read the version in test code (or no longer exist); \
         remove their lines:\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn a_new_test_reading_the_version_outside_the_set_is_named() {
    let needle = NEEDLES[0];
    let new_test = format!(
        "#[test]\nfn reports_the_version() {{\n    assert_eq!(version(), env!(\"{needle}\"));\n}}\n"
    );
    let file = "crates/rd-new/tests/about.rs";
    assert!(reads_the_version(file, &new_test));
    assert!(reads_the_version(
        "crates/rd-new/src/about_tests.rs",
        &new_test
    ));

    let list = std::fs::read_to_string(workspace_root().join(LIST)).expect("the list");
    let found = vec![file.to_owned()];
    assert_eq!(unlisted(&found, &listed(&list)), found, "named as missing");
}

#[test]
fn test_code_is_told_from_production_code() {
    let needle = NEEDLES[1];
    let read = format!("std::env::var(\"{needle}\")");
    let production = format!("pub fn version() -> String {{\n    {read}.unwrap_or_default()\n}}\n");
    let file = "crates/rd-new/src/about.rs";
    assert!(!reads_the_version(file, &production), "production code");

    let inline = format!(
        "{production}\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn t() {{\n        let _ = {read};\n    }}\n}}\n"
    );
    assert!(reads_the_version(file, &inline), "an inline test module");

    let gated_item = format!("#[cfg(test)]\nconst VERSION: &str = {read};\n");
    assert!(
        !reads_the_version(file, &gated_item),
        "a gated item is no test module"
    );

    let comment = format!("#[cfg(test)]\nmod tests {{\n    // {read}\n}}\n");
    assert!(!reads_the_version(file, &comment), "a comment");
}
