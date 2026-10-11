use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{CAP_BYTES, OWNERS_FILE, read_owners};
use crate::compile_cache::{WASMTIME_DIRECTORY, entries, hex_digest};
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

fn installed(components: &[&[u8]]) -> BTreeSet<String> {
    components.iter().map(|bytes| hex_digest(bytes)).collect()
}

/// Two components compiled, one after the other, as a start does.
fn two_compiled(root: &Path) -> SharedEngine {
    let shared = SharedEngine::new(Some(root)).expect("engine");
    shared.compile(&EMPTY_COMPONENT).expect("first");
    shared.compile(&variant(1)).expect("second");
    shared
}

#[test]
fn each_entry_is_recorded_with_the_component_that_wrote_it() {
    let root = tempfile::tempdir().expect("tempdir");
    two_compiled(root.path());
    let owners = read_owners(&root.path().join(OWNERS_FILE));
    assert_eq!(owners.entries.len(), 2);
    let mut written: Vec<_> = owners.entries.values().cloned().collect();
    written.sort();
    let mut expected = vec![installed(&[&EMPTY_COMPONENT]), installed(&[&variant(1)])];
    expected.sort();
    assert_eq!(written, expected, "one owner each, never the other's");
    assert!(
        owners
            .compiler_directory
            .as_deref()
            .is_some_and(|directory| directory.starts_with("wasmtime-")),
        "{owners:?}"
    );
}

#[test]
fn an_entry_no_installed_component_owns_goes_and_compiles_again_if_needed() {
    let root = tempfile::tempdir().expect("tempdir");
    let shared = two_compiled(root.path());
    let disk = shared.disk().expect("a disk cache");
    let only_first = installed(&[&EMPTY_COMPONENT]);

    let preview = disk.prune(&only_first, CAP_BYTES, true);
    assert_eq!((preview.kept_entries, preview.removed_entries), (1, 1));
    assert_eq!(
        entry_paths(root.path()).len(),
        2,
        "a dry run removes nothing"
    );

    let pruned = disk.prune(&only_first, CAP_BYTES, false);
    assert_eq!(pruned, preview, "the pass does what its preview said");
    assert!(pruned.removed_bytes > 0);
    assert_eq!(entry_paths(root.path()).len(), 1);
    drop(shared);

    // The next start reads the installed one back and has to compile the other.
    let next = SharedEngine::new(Some(root.path())).expect("engine");
    next.compile(&EMPTY_COMPONENT).expect("installed");
    next.compile(&variant(1)).expect("removed one");
    let cache = next.disk().expect("a disk cache").cache();
    assert_eq!((cache.cache_hits(), cache.cache_misses()), (1, 1));
}

#[test]
fn the_cap_keeps_the_most_recently_used_entries() {
    let root = tempfile::tempdir().expect("tempdir");
    let shared = two_compiled(root.path());
    let disk = shared.disk().expect("a disk cache");
    let both = installed(&[&EMPTY_COMPONENT, &variant(1)]);
    assert_eq!(disk.prune(&both, CAP_BYTES, true).removed_entries, 0);

    // The second component's entry is the one used last.
    let owners = read_owners(&root.path().join(OWNERS_FILE));
    let modules = root.path().join(WASMTIME_DIRECTORY).join("modules");
    let recent = owners
        .entries
        .iter()
        .find(|(_, components)| **components == installed(&[&variant(1)]))
        .map(|(key, _)| modules.join(key))
        .expect("the second component's entry");
    std::fs::File::options()
        .write(true)
        .open(&recent)
        .expect("entry")
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .expect("mtime");
    let sizes: u64 = entry_paths(root.path())
        .iter()
        .map(|path| std::fs::metadata(path).expect("entry").len())
        .sum();
    // Room for either entry with its usage file, never for both.
    let pruned = disk.prune(&both, sizes - 1, false);
    assert_eq!((pruned.kept_entries, pruned.removed_entries), (1, 1));
    assert_eq!(
        entry_paths(root.path()),
        vec![recent],
        "the least recently used goes"
    );

    let nothing = disk.prune(&both, 0, false);
    assert_eq!(nothing.kept_entries, 0);
    assert!(entry_paths(root.path()).is_empty());
}

#[test]
fn another_wasmtime_release_and_entries_without_owners_go() {
    let root = tempfile::tempdir().expect("tempdir");
    let shared = two_compiled(root.path());
    let modules = root.path().join(WASMTIME_DIRECTORY).join("modules");
    let older = modules.join("wasmtime-0.0.1");
    std::fs::create_dir_all(&older).expect("older release");
    std::fs::write(older.join("B".repeat(43)), vec![1_u8; 300]).expect("old entry");
    std::fs::write(older.join(format!("{}.stats", "B".repeat(43))), b"usages").expect("stats");
    // An entry recorded by a build before the owners were: it counts as nobody's.
    let mut owners = read_owners(&root.path().join(OWNERS_FILE));
    let unowned = owners.entries.keys().next().cloned().expect("an entry");
    owners.entries.remove(&unowned);
    *shared
        .disk()
        .expect("a disk cache")
        .owners
        .lock()
        .expect("owners") = owners;

    let both = installed(&[&EMPTY_COMPONENT, &variant(1)]);
    let pruned = shared
        .disk()
        .expect("a disk cache")
        .prune(&both, CAP_BYTES, false);
    assert_eq!(pruned.kept_entries, 1);
    assert_eq!(
        pruned.removed_entries, 2,
        "the older release's and the unowned one"
    );
    assert!(pruned.removed_bytes >= 306);
    assert!(!older.exists());
    assert!(!modules.join(&unowned).exists());
    let stored = read_owners(&root.path().join(OWNERS_FILE));
    assert_eq!(stored.entries.len(), 1, "the record follows the files");
}

#[test]
fn a_record_without_a_current_release_leaves_the_directories_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let shared = SharedEngine::new(Some(root.path())).expect("engine");
    let older = root
        .path()
        .join(WASMTIME_DIRECTORY)
        .join("modules")
        .join("wasmtime-0.0.1");
    std::fs::create_dir_all(&older).expect("older release");
    std::fs::write(older.join("C".repeat(43)), b"unrecorded").expect("entry");
    let pruned = shared
        .disk()
        .expect("a disk cache")
        .prune(&BTreeSet::new(), CAP_BYTES, false);
    assert_eq!(pruned.removed_entries, 0);
    assert_eq!(pruned.kept_entries, 1, "unrecorded, so never a candidate");
    assert!(older.exists());
}
