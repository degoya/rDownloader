//! No plugin crate bakes its own path into a binary.
//!
//! `env!("CARGO_MANIFEST_DIR")` is resolved when the code *compiles*. The worktrees share one
//! target directory, and `plugins/` is deliberately never stamped -- stamping it would make every
//! component look stale to `scripts/build-plugins.sh`. So a plugin's test binary, built once in
//! one worktree, is reused by every other, still carrying the first worktree's path. The moment
//! that worktree is removed, the test fails with "No such file or directory" in a branch that
//! never touched the plugin.
//!
//! It happened on 2026-09-23: `plugins/premiumize-transfers/tests/key_canary.rs` read its own
//! sources through `concat!(env!("CARGO_MANIFEST_DIR"), …)`, the worktree that built it had been
//! merged and removed, and an unrelated branch's full run stopped on it. The same pattern is
//! harmless under `crates/`, because `rd_take_lock` stamps those sources and so rebuilds them in
//! the checkout under test. Under `plugins/` nothing does.
//!
//! Read the variable at run time instead -- `std::env::var_os("CARGO_MANIFEST_DIR")` -- which the
//! test runner sets for the checkout actually being tested.

use std::path::{Path, PathBuf};

const NEEDLE: &str = "env!(\"CARGO_MANIFEST_DIR\")";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

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

#[test]
fn no_plugin_resolves_its_path_at_compile_time() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root.join("plugins"), &mut files);
    assert!(!files.is_empty(), "no plugin sources were found at all");

    let offenders: Vec<String> = files
        .iter()
        .filter(|path| std::fs::read_to_string(path).is_ok_and(|text| text.contains(NEEDLE)))
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "these plugin sources bake their own path in with {NEEDLE}, which breaks once the \
         worktree that compiled them is gone; read std::env::var_os(\"CARGO_MANIFEST_DIR\") at \
         run time instead:\n  {}",
        offenders.join("\n  ")
    );
}
