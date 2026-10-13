//! The eight crates of the HTTP surface depend on each other only along the allowed edges.
//!
//! RD-160-06 split one 63,500-line `rd-api` into eight crates so that rustc compiles the four
//! areas side by side once `rd-api-core` is done. That holds only while no area depends on
//! another: one sideways edge serialises two areas again, and a change to one handler rebuilds
//! its neighbour. The rule stood in `AGENTS.md` and `docs/architecture.md#rd-api` with nothing
//! holding it (audit 2026-10-13, RD-1280-16). A helper two areas share goes down into
//! `rd-api-core`, never sideways.
//!
//! Two crates sit above the areas by design, and their edges are written down here rather than
//! tolerated: `rd-api-compat` translates the adapters' calls onto the native handlers of intake
//! and queue, and `rd-api-mcp` calls the same handlers the REST routes do, in all four areas.
//! `rd-api` is the assembly and may name every one of them. Every dependency section counts --
//! normal, build, dev and target-specific -- because a dev-dependency edge orders the test build
//! the same way.

use std::path::Path;

use crate::workspace_root;

/// Each crate of the HTTP surface with the `rd-api*` crates it may depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    ("rd-api-core", &[]),
    ("rd-api-access", &["rd-api-core"]),
    ("rd-api-admin", &["rd-api-core"]),
    ("rd-api-intake", &["rd-api-core"]),
    ("rd-api-queue", &["rd-api-core"]),
    // Exception: the adapters translate onto the native intake and queue handlers.
    (
        "rd-api-compat",
        &["rd-api-core", "rd-api-intake", "rd-api-queue"],
    ),
    // Exception: the MCP tools call the handlers of all four areas.
    (
        "rd-api-mcp",
        &[
            "rd-api-core",
            "rd-api-access",
            "rd-api-admin",
            "rd-api-intake",
            "rd-api-queue",
        ],
    ),
    (
        "rd-api",
        &[
            "rd-api-core",
            "rd-api-access",
            "rd-api-admin",
            "rd-api-intake",
            "rd-api-queue",
            "rd-api-compat",
            "rd-api-mcp",
        ],
    ),
];

/// The leading crate name of a manifest line or key: letters, digits and dashes.
fn crate_name(text: &str) -> String {
    text.chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect()
}

/// The `rd-api*` crates a manifest names in any dependency section, in the forms
/// `rd-api-x = …`, `rd-api-x.workspace = true` and `[dependencies.rd-api-x]`.
fn http_dependencies(manifest: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(manifest)
        .unwrap_or_else(|error| panic!("{}: {error}", manifest.display()));
    let mut in_dependencies = false;
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let named = if line.starts_with('[') {
            let header = line.trim_start_matches('[').trim_end_matches(']');
            in_dependencies = header.ends_with("dependencies");
            // A table of its own: `[dependencies.rd-api-x]`, `[target.'…'.dependencies.rd-api-x]`.
            header
                .split_once("dependencies.")
                .map(|(_, rest)| crate_name(rest))
        } else if in_dependencies {
            Some(crate_name(line))
        } else {
            None
        };
        if let Some(name) = named
            && (name == "rd-api" || name.starts_with("rd-api-"))
            && !found.contains(&name)
        {
            found.push(name);
        }
    }
    found
}

#[test]
fn the_table_names_every_http_crate() {
    let root = workspace_root();
    let mut on_disk: Vec<String> = std::fs::read_dir(root.join("crates"))
        .expect("crates directory")
        .flatten()
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name == "rd-api" || name.starts_with("rd-api-"))
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = ALLOWED.iter().map(|(name, _)| (*name).to_owned()).collect();
    listed.sort();
    assert_eq!(
        on_disk, listed,
        "the crates of the HTTP surface changed; give a new one its row in ALLOWED (an area \
         depends on rd-api-core alone) and drop the row of one that is gone"
    );
}

#[test]
fn no_http_crate_depends_outside_its_allowed_edges() {
    let root = workspace_root();
    let mut offenders = Vec::new();
    for (name, allowed) in ALLOWED {
        let manifest = root.join("crates").join(name).join("Cargo.toml");
        for dependency in http_dependencies(&manifest) {
            if !allowed.contains(&dependency.as_str()) {
                offenders.push(format!("{name} -> {dependency}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these edges serialise the HTTP crates' build (RD-160-06); move the shared helper down \
         into rd-api-core instead of depending sideways, or, if the edge is a deliberate \
         exception like rd-api-compat's and rd-api-mcp's, write it into ALLOWED with its \
         reason:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_reader_sees_the_edges_that_exist() {
    let root = workspace_root();
    let mcp = http_dependencies(&root.join("crates/rd-api-mcp/Cargo.toml"));
    assert!(
        mcp.iter().any(|name| name == "rd-api-intake"),
        "the manifest reader no longer finds rd-api-mcp's dependency on rd-api-intake, so it \
         would pass any graph: {mcp:?}"
    );
}
