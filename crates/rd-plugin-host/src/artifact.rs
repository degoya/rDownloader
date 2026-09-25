//! Loading a built plugin component for a test, and refusing one that is missing or out of
//! date.
//!
//! Every contract test in the workspace reads its component from
//! `target/wasm32-unknown-unknown/release/`. That directory is per checkout, and
//! `cargo test` never builds it: `cargo component build` does, separately and by hand. So a
//! worktree that changes a plugin and is then merged leaves the main checkout holding the
//! *previous* component while its sources have already moved on — and the contract tests run
//! the old guest code against the new expectations.
//!
//! What made that expensive twice is that the failure never looked like what it was:
//!
//! * after RD-106-01, `instance export 'rdownloader:plugin/oauth@0.6.0' does not have export
//!   'device-begin'` — the component predated the contract extension;
//! * after RD-106-04, `left: Some("rdownloader-google-drive.apps.googleusercontent.invalid"),
//!   right: Some("{{client_id}}")` — the component still carried the identifier the job had
//!   just removed.
//!
//! Both read as a defect in the code under test. Both were an artefact nobody had rebuilt.
//!
//! This module makes that state name itself. Before a component is handed to a test it is
//! compared with the sources it was built from; if they differ the test fails with the build
//! command instead of with an assertion about behaviour. The check hashes a handful of small
//! files and starts no build — a test that shelled out to `cargo component build` would make
//! every run pay for the one case this guards against.
//!
//! The comparison is by content (RD-120-58). `scripts/build-plugins.sh` writes a stamp beside
//! every component it builds, `<artefact>.wasm.src-sha256`, holding two hashes: the source hash
//! (below) and the SHA-256 of the component itself. A component is current when the stamp
//! exists, names exactly these component bytes and records exactly the current source hash.
//! Until then the rule compared modification times, and every `git checkout` resets those: in
//! five of eight branch checks on 2026-09-24 a fresh worktree reported components stale that no
//! one had changed. A component without a stamp, or whose bytes the stamp does not describe —
//! a bare `cargo component build`, possibly in another checkout sharing `target/` — is stale:
//! a stamp only vouches for a build it saw.
//!
//! The source hash is the SHA-256 of a `sha256sum` listing: one `<hex>  <path>\n` line per
//! source file, the path relative to the workspace root with `/` separators, sorted bytewise.
//! No file time and no checkout location enters it. `build-plugins.sh --source-hash <name>`
//! computes the same value, and a test holds the two to it.
//!
//! A component that was never built is the same kind of state, and until RD-108-16 it was the
//! quiet one: the loader returned `None`, every caller returned early, and a run of thirty
//! contract tests reported thirty greens without a single one of them ever entering a
//! component. Nextest hides the stderr of a passing test, so the `SKIPPED` line that said so
//! was never read. Both states now fail, and for the same reason: a test that does not run
//! guest code proves nothing about guest code, and a number that cannot tell the two apart is
//! worth nothing either.
//!
//! A checkout that cannot build components at all — no `wasm32-unknown-unknown` target, no
//! `cargo-component` — deselects these tests explicitly with the `no-components` nextest
//! profile (`.config/nextest.toml`), which counts them as *skipped*. That is the whole
//! difference: skipping is a decision somebody takes and the summary shows, not something the
//! loader does behind a green number.
//!
//! The source set is deliberately narrow: the plugin's own crate, any path dependency of it
//! that lives under `plugins/`, and the WIT contract in `crates/rd-plugin-api/wit`. Path
//! dependencies that point into `crates/` are *not* followed. A component built from
//! `rd-core` really does go stale when `rd-core` changes, but saying so would mark half the
//! components stale on an ordinary day's work in the core, and a check that cries wolf is one
//! nobody reads. The two shapes actually observed — the contract grew, the plugin's own source
//! changed — are both covered.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where `cargo component build --release --target wasm32-unknown-unknown` leaves its output.
const ARTIFACT_DIRECTORY: &str = "wasm32-unknown-unknown/release";

/// The WIT contract every component is generated against.
const CONTRACT_DIRECTORY: &str = "crates/rd-plugin-api/wit";

/// File names, beyond `*.rs` and `*.wit`, that a component is built from.
const BUILD_INPUTS: [&str; 2] = ["Cargo.toml", "manifest.toml"];

/// What cargo-component writes into `src/` of a plugin crate at every build (gitignored). It is
/// generated from the WIT, which is a source already, and a checkout that never built the plugin
/// does not have it — so it is not a source, or a fresh worktree would disagree with every stamp.
const GENERATED_BINDINGS: &str = "bindings.rs";

/// Appended to the artefact's file name for the stamp `scripts/build-plugins.sh` writes.
const STAMP_SUFFIX: &str = ".src-sha256";

/// The workspace root, derived from this crate's own location.
///
/// Resolved where the filesystem allows it, because it otherwise travels through `../..` and
/// every path built from it reads badly in a failure message.
fn workspace_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

/// The directory cargo builds into, honouring `CARGO_TARGET_DIR`.
fn target_directory() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

/// The `plugins/` directory name of a package, following the workspace's
/// `rd-plugin-<directory>` naming — the same mapping `scripts/build-plugins.sh` relies on.
fn plugin_name(package: &str) -> &str {
    package.strip_prefix("rd-plugin-").unwrap_or(package)
}

/// `plugins/<directory>` for a package, under `root`.
fn plugin_directory(root: &Path, package: &str) -> PathBuf {
    root.join("plugins").join(plugin_name(package))
}

/// The artefact `cargo component build` writes for a package.
fn artifact_path(package: &str) -> PathBuf {
    target_directory()
        .join(ARTIFACT_DIRECTORY)
        .join(format!("{}.wasm", package.replace('-', "_")))
}

/// The stamp beside an artefact.
fn stamp_path(artifact: &Path) -> PathBuf {
    let mut name = artifact.as_os_str().to_owned();
    name.push(STAMP_SUFFIX);
    PathBuf::from(name)
}

/// The command that builds and stamps this component.
fn build_command(package: &str) -> String {
    format!(
        "scripts/build-plugins.sh --components-only {}",
        plugin_name(package)
    )
}

/// A built component, or a failure naming the command that builds it.
///
/// `package` is the cargo package name, such as `rd-plugin-example-oauth`.
///
/// The wasm target is not part of an ordinary `cargo test`, so the components are built
/// separately — by `scripts/build-plugins.sh` here and by the `components` job in CI. Neither
/// way of not having the right one is survivable: a **missing** artefact fails, because a
/// caller that carried on would report a green for guest code it never ran, and a **stale**
/// one fails, because it would be tested as if it were current.
#[must_use]
pub fn component(package: &str) -> Vec<u8> {
    let path = artifact_path(package);
    let Ok(bytes) = std::fs::read(&path) else {
        panic!("{}", missing(package, &path));
    };
    let stamp = std::fs::read_to_string(stamp_path(&path)).ok();
    let current = source_hash(&workspace_root(), package);
    if let Some(complaint) = staleness(package, &path, &bytes, stamp.as_deref(), &current) {
        panic!("{complaint}");
    }
    bytes
}

/// What to say about a component that has not been built in this checkout.
fn missing(package: &str, path: &Path) -> String {
    format!(
        "{} has not been built in this checkout.\n\
         \n  \
         expected: {}\n\
         \n\
         This test drives the real guest code, so without the component there is nothing for \
         it to check. Returning early here is what let a full contract run report nothing but \
         greens while no component was ever loaded (RD-108-16). Build it:\n\
         \n  {}\n\
         \n\
         Without a name the same script builds every missing or stale component, with the job \
         count this machine needs. A checkout without the wasm toolchain leaves these tests out \
         on purpose, with `cargo nextest run -P no-components`, which counts them as skipped \
         rather than passed.",
        path.file_name().unwrap_or_default().to_string_lossy(),
        path.display(),
        build_command(package),
    )
}

/// What to say about the component `bytes` at `path` when its `stamp` does not prove it was
/// built from sources hashing to `current`, or `None` when it does.
///
/// Everything read from disk arrives as a parameter, so a test can put a stamp or a source hash
/// of its choosing in front of the rule without touching the shared target directory.
fn staleness(
    package: &str,
    path: &Path,
    bytes: &[u8],
    stamp: Option<&str>,
    current: &str,
) -> Option<String> {
    let recorded = stamp.and_then(|text| {
        let mut fields = text.split_whitespace();
        Some((fields.next()?, fields.next()?))
    });
    let reason = match recorded {
        None => "it carries no source stamp, so nothing says what it was built from".to_owned(),
        Some((_, component)) if component != hex_sha256(bytes) => {
            "its stamp describes other bytes: the component was rebuilt without a stamp, by a \
             bare `cargo component build`, possibly in another checkout sharing this target"
                .to_owned()
        }
        Some((sources, _)) if sources != current => format!(
            "its sources changed since it was built (stamp {}, sources now {})",
            sources.get(..12).unwrap_or(sources),
            current.get(..12).unwrap_or(current),
        ),
        Some(_) => return None,
    };
    Some(format!(
        "{} is not the component its current sources produce.\n\
         \n  \
         component: {}\n  \
         why:       {reason}\n\
         \n\
         That is a stale artefact, not a defect in the code under test. `cargo test` never \
         builds components and target/ is shared between checkouts, so this test would \
         otherwise run other guest code against the current expectations. Rebuild it:\n\
         \n  {}\n\
         \n\
         Without a name the same script rebuilds every missing or stale component, with the \
         job count this machine needs.",
        path.file_name().unwrap_or_default().to_string_lossy(),
        path.display(),
        build_command(package),
    ))
}

/// Lower-case hex of the SHA-256 of `bytes`.
fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The source hash of `package` in the workspace at `root`, as the module documentation
/// defines it. An unreadable file contributes a line no real content hashes to, so it can never
/// match a stamp.
fn source_hash(root: &Path, package: &str) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut files: Vec<(String, PathBuf)> = sources(&root, package)
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(&root).ok()?;
            let parts: Vec<_> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            Some((parts.join("/"), path))
        })
        .collect();
    files.sort();
    files.dedup();
    let listing: String = files
        .iter()
        .map(|(relative, path)| {
            let digest =
                std::fs::read(path).map_or_else(|_| "unreadable".to_owned(), |b| hex_sha256(&b));
            format!("{digest}  {relative}\n")
        })
        .collect();
    hex_sha256(listing.as_bytes())
}

/// Every file the component for `package` is built from, within the bounds the module
/// documentation describes.
fn sources(root: &Path, package: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let plugins = root.join("plugins");
    let mut pending = vec![plugin_directory(root, package)];
    let mut seen: Vec<PathBuf> = Vec::new();

    while let Some(directory) = pending.pop() {
        let Ok(directory) = directory.canonicalize() else {
            continue;
        };
        if seen.contains(&directory) {
            continue;
        }
        seen.push(directory.clone());
        collect(&directory, &mut files);
        for dependency in path_dependencies(&directory) {
            // Only the shared plugin libraries; see the module documentation for why the
            // trail stops at the edge of `plugins/`.
            if plugins
                .canonicalize()
                .is_ok_and(|plugins| dependency.starts_with(&plugins))
            {
                pending.push(dependency);
            }
        }
    }

    let contract = root.join(CONTRACT_DIRECTORY);
    collect(&contract.canonicalize().unwrap_or(contract), &mut files);
    files
}

/// The directories named by `path = "…"` entries in a crate's `Cargo.toml`, resolved against
/// it.
///
/// A line scan rather than a parse: the entries this looks for are one per line in every
/// manifest in the tree, and the caller discards anything outside `plugins/` anyway — so the
/// `[package.metadata.component.target]` path to the WIT directory falling in here is
/// harmless.
fn path_dependencies(directory: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(directory.join("Cargo.toml")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once("path = \"")?;
            let (value, _) = rest.split_once('"')?;
            directory.join(value).canonicalize().ok()
        })
        .collect()
}

/// Adds every build input under `directory` to `files`. Symbolic links are neither followed
/// nor collected, as `find -type f` in the script does not.
fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, files);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let extension = path.extension().and_then(|value| value.to_str());
        let name = path.file_name().and_then(|value| value.to_str());
        let generated = name == Some(GENERATED_BINDINGS)
            && path
                .parent()
                .is_some_and(|parent| parent.file_name().is_some_and(|dir| dir == "src"))
            && path
                .ancestors()
                .nth(3)
                .is_some_and(|plugins| plugins.ends_with("plugins"));
        if generated {
            continue;
        }
        if matches!(extension, Some("rs" | "wit"))
            || name.is_some_and(|n| BUILD_INPUTS.contains(&n))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
#[path = "artifact_tests.rs"]
mod tests;
