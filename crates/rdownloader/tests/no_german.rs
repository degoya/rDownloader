//! Guards the English-only rule for Rust sources: user-facing texts live in code with a
//! translation code, translations live in the web catalogues. Any German umlaut or sharp s
//! inside a Rust source (outside explicitly allow-listed fixtures) is a regression.

use std::path::{Path, PathBuf};

const ROOTS: &[&str] = &["crates", "plugins"];
const ALLOWED_FILES: &[&str] = &[
    "crates/rd-api/src/link_check_probe.rs",
    "crates/rdownloader/tests/no_german.rs",
];
const GERMAN_CHARS: &str = "\u{e4}\u{f6}\u{fc}\u{c4}\u{d6}\u{dc}\u{df}";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn rust_sources(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn rust_sources_contain_no_german_text() {
    let root = workspace_root();
    let mut sources = Vec::new();
    for dir in ROOTS {
        rust_sources(&root.join(dir), &mut sources);
    }
    assert!(sources.len() > 50, "expected to scan the workspace sources");
    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| {
            let relative = path.strip_prefix(&root).expect("relative");
            // `Path` equality compares components, so the `/` spelling matches a Windows `\`.
            !ALLOWED_FILES
                .iter()
                .any(|allowed| relative == Path::new(allowed))
        })
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            let line = text
                .lines()
                .enumerate()
                .find(|(_, line)| line.chars().any(|c| GERMAN_CHARS.contains(c)))?;
            Some(format!(
                "{}:{}: {}",
                path.display(),
                line.0 + 1,
                line.1.trim()
            ))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "German text found:\n{}",
        offenders.join("\n")
    );
}
