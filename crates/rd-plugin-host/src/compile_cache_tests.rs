use std::path::{Path, PathBuf};

use super::{INDEX_FILE, WASMTIME_DIRECTORY, entries};
use crate::engine::{
    SharedEngine,
    tests::{EMPTY_COMPONENT, variant},
};

fn entry_paths(root: &Path) -> Vec<PathBuf> {
    entries(&root.join(WASMTIME_DIRECTORY).join("modules"))
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// (hits, misses) of the engine's disk cache so far.
fn counts(shared: &SharedEngine) -> (usize, usize) {
    let cache = shared.disk().expect("a disk cache").cache();
    (cache.cache_hits(), cache.cache_misses())
}

/// Compiles one component in an engine of its own and drops it again, as a start would.
fn compile_once(root: &Path, bytes: &[u8]) -> (usize, usize) {
    let shared = SharedEngine::new(Some(root)).expect("engine");
    shared.compile(bytes).expect("compile");
    counts(&shared)
}

/// The Wasmtime release this build links, read from the lock file at run time.
fn locked_wasmtime_version() -> String {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("run under cargo");
    let lock = std::fs::read_to_string(Path::new(&manifest).join("../../Cargo.lock"))
        .expect("workspace lock file");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == r#"name = "wasmtime""# {
            let version = lines.next().expect("a version line");
            return version
                .trim_start_matches("version = \"")
                .trim_end_matches('"')
                .to_owned();
        }
    }
    panic!("no wasmtime package in Cargo.lock");
}

/// The point of the job: a restart with unchanged plugins reads them back.
#[test]
fn an_unchanged_component_is_read_back_instead_of_compiled() {
    let root = tempfile::tempdir().expect("tempdir");
    assert_eq!(compile_once(root.path(), &EMPTY_COMPONENT), (0, 1));
    assert_eq!(
        compile_once(root.path(), &EMPTY_COMPONENT),
        (1, 0),
        "the second start must not compile"
    );
}

#[test]
fn a_changed_component_compiles_anew() {
    let root = tempfile::tempdir().expect("tempdir");
    compile_once(root.path(), &EMPTY_COMPONENT);
    assert_eq!(
        compile_once(root.path(), &variant(1)),
        (0, 1),
        "different bytes are a different entry"
    );
    assert_eq!(entry_paths(root.path()).len(), 2);
}

/// Wasmtime keeps each release's entries in a directory of its own, so an update never reads
/// code the previous release compiled. The test pins that layout: if a Wasmtime update moves
/// it, this fails and the claim in `compile_cache.rs` has to be checked again.
#[test]
fn an_entry_is_bound_to_the_wasmtime_version() {
    let root = tempfile::tempdir().expect("tempdir");
    compile_once(root.path(), &EMPTY_COMPONENT);
    let paths = entry_paths(root.path());
    assert_eq!(paths.len(), 1);
    let version_directory = paths[0]
        .parent()
        .and_then(Path::file_name)
        .expect("a version directory")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        version_directory,
        format!("wasmtime-{}", locked_wasmtime_version())
    );
}

/// A damaged entry could decompress into different machine code, which would run outside the
/// sandbox. It must be thrown away before the engine can read it, and the component compiled.
#[test]
fn a_damaged_entry_is_compiled_again_rather_than_run() {
    let root = tempfile::tempdir().expect("tempdir");
    compile_once(root.path(), &EMPTY_COMPONENT);
    let entry = entry_paths(root.path()).pop().expect("an entry");
    let mut damaged = std::fs::read(&entry).expect("entry bytes");
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x01;
    std::fs::write(&entry, &damaged).expect("damage the entry");

    let shared = SharedEngine::new(Some(root.path())).expect("a damaged entry is no failure");
    assert!(
        !entry.exists(),
        "the damaged entry must be gone before anything compiles"
    );
    shared.compile(&EMPTY_COMPONENT).expect("compiles again");
    assert_eq!(counts(&shared), (0, 1));
    assert!(entry.exists(), "the fresh compile writes the entry again");
}

/// An entry the service did not write itself — copied in, or left without its record — is
/// not trusted either, however intact it looks.
#[test]
fn an_entry_without_a_record_is_removed() {
    let root = tempfile::tempdir().expect("tempdir");
    compile_once(root.path(), &EMPTY_COMPONENT);
    let entry = entry_paths(root.path()).pop().expect("an entry");
    let planted = entry
        .parent()
        .expect("version directory")
        .join("A".repeat(super::ENTRY_NAME_LENGTH));
    std::fs::copy(&entry, &planted).expect("plant an entry");
    std::fs::remove_file(root.path().join(INDEX_FILE)).expect("drop the record");

    let shared = SharedEngine::new(Some(root.path())).expect("engine");
    assert!(!entry.exists(), "an unrecorded entry is removed");
    assert!(!planted.exists(), "a planted entry is removed");
    shared.compile(&EMPTY_COMPONENT).expect("compiles again");
    assert_eq!(counts(&shared), (0, 1));
}

#[test]
fn an_unreadable_record_costs_compiles_not_the_start() {
    let root = tempfile::tempdir().expect("tempdir");
    compile_once(root.path(), &EMPTY_COMPONENT);
    std::fs::write(root.path().join(INDEX_FILE), b"{ not json").expect("damage the record");

    assert_eq!(compile_once(root.path(), &EMPTY_COMPONENT), (0, 1));
    assert_eq!(
        compile_once(root.path(), &EMPTY_COMPONENT),
        (1, 0),
        "the record is whole again after one start"
    );
}
