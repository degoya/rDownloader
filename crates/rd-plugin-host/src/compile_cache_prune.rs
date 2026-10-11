//! Thinning the compile cache out (RD-1240-34).
//!
//! Wasmtime keeps an entry for every component it ever compiled, under a directory per Wasmtime
//! release, and forgets one only past its own size limit. An updated plugin, a removed one or a
//! Wasmtime update therefore left its entries for good. A pass here removes, in this order:
//!
//! 1. every directory of another Wasmtime release than the one this build writes to, once the
//!    record knows that one (it learns it from the first entry the build writes);
//! 2. every recorded entry none of whose components is installed — an entry recorded before
//!    the owners were (an older build's) counts as such and compiles again once;
//! 3. the least recently used of what is left, until it fits [`CAP_BYTES`].
//!
//! Only recorded entries are candidates, and the pass holds the record's lock, so an entry a
//! compile is still writing — not recorded yet — is never touched. Removing an entry is never
//! more than a compile: the next load of its component writes it again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{CompileCache, is_entry_name, write_index};

/// The record of owners, beside the integrity record.
pub(crate) const OWNERS_FILE: &str = "owners.json";

/// What the cache may hold after a pass: 256 MiB, about three times what the 72 bundled plugins
/// compile to.
pub(crate) const CAP_BYTES: u64 = 256 * 1024 * 1024;

/// Wasmtime's usage file beside an entry.
const STATS_SUFFIX: &str = ".stats";

/// Whose entries are whose.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Owners {
    /// The Wasmtime directory below `modules/` this build writes to, once it wrote an entry.
    #[serde(default)]
    pub(crate) compiler_directory: Option<String>,
    /// Entry path to the hex SHA-256 of the components that were compiling when it appeared.
    #[serde(default)]
    pub(crate) entries: BTreeMap<String, BTreeSet<String>>,
}

/// What a pass found or did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CachePrune {
    pub kept_entries: u64,
    pub kept_bytes: u64,
    /// In a dry run what would go; otherwise what went.
    pub removed_entries: u64,
    pub removed_bytes: u64,
}

/// One recorded entry in the directory this build writes to.
struct Entry {
    key: String,
    path: std::path::PathBuf,
    /// The entry and its usage file.
    bytes: u64,
    /// The later of the two files' times: Wasmtime touches the usage file on every read.
    used: SystemTime,
}

impl CompileCache {
    /// One pass over the cache; see the module documentation. `installed` holds the hex
    /// SHA-256 of every installed component.
    pub(crate) fn prune(
        &self,
        installed: &BTreeSet<String>,
        cap: u64,
        dry_run: bool,
    ) -> CachePrune {
        let Ok(mut index) = self.index.lock() else {
            return CachePrune::default();
        };
        let Ok(mut owners) = self.owners.lock() else {
            return CachePrune::default();
        };
        let mut result = CachePrune::default();
        let current = owners.compiler_directory.clone();
        let mut candidates = Vec::new();
        for version in read_dir(&self.modules) {
            let name = version.file_name().to_string_lossy().into_owned();
            let is_current = current.as_deref().is_none_or(|current| current == name);
            if !is_current {
                let (entries, bytes) = measure_directory(&version.path());
                if dry_run || std::fs::remove_dir_all(version.path()).is_ok() {
                    result.removed_entries += entries;
                    result.removed_bytes += bytes;
                    if !dry_run {
                        let prefix = format!("{name}/");
                        index.retain(|key, _| !key.starts_with(&prefix));
                    }
                } else {
                    result.kept_entries += entries;
                    result.kept_bytes += bytes;
                }
                continue;
            }
            for file in read_dir(&version.path()) {
                let file_name = file.file_name().to_string_lossy().into_owned();
                let key = format!("{name}/{file_name}");
                if !is_entry_name(&file_name) {
                    continue;
                }
                let Some(metadata) = file.metadata().ok().filter(std::fs::Metadata::is_file) else {
                    continue;
                };
                let stats = std::fs::metadata(stats_path(&file.path())).ok();
                let used = [
                    metadata.modified().ok(),
                    stats.as_ref().and_then(|stats| stats.modified().ok()),
                ]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH);
                let entry = Entry {
                    bytes: metadata.len() + stats.map_or(0, |stats| stats.len()),
                    path: file.path(),
                    key,
                    used,
                };
                if !index.contains_key(&entry.key) {
                    // Being written by a compile right now, or left for the next start's check.
                    result.kept_entries += 1;
                    result.kept_bytes += entry.bytes;
                    continue;
                }
                candidates.push(entry);
            }
        }
        // Owned by an installed component first, the most recently used of them first.
        let (mut owned, orphans): (Vec<Entry>, Vec<Entry>) =
            candidates.into_iter().partition(|entry| {
                owners
                    .entries
                    .get(&entry.key)
                    .is_some_and(|components| !components.is_disjoint(installed))
            });
        owned.sort_by_key(|entry| std::cmp::Reverse(entry.used));
        let mut total = result.kept_bytes;
        let mut removable = orphans;
        for entry in owned {
            if total + entry.bytes > cap {
                removable.push(entry);
            } else {
                total += entry.bytes;
                result.kept_entries += 1;
                result.kept_bytes += entry.bytes;
            }
        }
        for entry in removable {
            if !dry_run && remove_entry(&entry.path).is_err() {
                result.kept_entries += 1;
                result.kept_bytes += entry.bytes;
                continue;
            }
            result.removed_entries += 1;
            result.removed_bytes += entry.bytes;
            if !dry_run {
                index.remove(&entry.key);
            }
        }
        if !dry_run {
            owners.entries.retain(|key, _| index.contains_key(key));
            if let Err(error) = write_index(&self.index_path, &index) {
                tracing::warn!(%error, "could not record the plugin compile cache");
            }
            if let Err(error) = write_owners(&self.owners_path, &owners) {
                tracing::warn!(%error, "could not record whose plugin cache entries are whose");
            }
        }
        result
    }
}

fn read_dir(directory: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(directory)
        .map(|entries| entries.flatten().collect())
        .unwrap_or_default()
}

fn stats_path(entry: &Path) -> std::path::PathBuf {
    let mut path = entry.as_os_str().to_owned();
    path.push(STATS_SUFFIX);
    path.into()
}

/// An entry and its usage file; a usage file already gone is no failure.
fn remove_entry(entry: &Path) -> std::io::Result<()> {
    std::fs::remove_file(entry)?;
    match std::fs::remove_file(stats_path(entry)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The entries in a directory of another Wasmtime release, and all the bytes in it.
fn measure_directory(directory: &Path) -> (u64, u64) {
    let mut entries = 0;
    let mut bytes = 0;
    for file in read_dir(directory) {
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let (more_entries, more_bytes) = measure_directory(&file.path());
            entries += more_entries;
            bytes += more_bytes;
            continue;
        }
        bytes += metadata.len();
        if is_entry_name(&file.file_name().to_string_lossy()) {
            entries += 1;
        }
    }
    (entries, bytes)
}

/// The stored owners; an unreadable record is an empty one, which costs one compile per entry.
pub(crate) fn read_owners(path: &Path) -> Owners {
    let Ok(bytes) = std::fs::read(path) else {
        return Owners::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, "the plugin cache's record of owners is unreadable; starting it afresh");
        Owners::default()
    })
}

/// Writes the owners through a temporary file, so a crash leaves the old record or the new one.
pub(crate) fn write_owners(path: &Path, owners: &Owners) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(owners)?)
        .with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

/// One pass over the compile cache the service configured, by the components installed now:
/// the hex SHA-256 of each, as [`crate::PluginInstaller::installed_component_digests`] lists
/// them. Without a configured cache — the CLI, the tests — there is nothing to prune.
#[must_use]
pub fn prune_compile_cache(installed: &BTreeSet<String>, dry_run: bool) -> CachePrune {
    crate::engine::configured()
        .and_then(|shared| {
            shared
                .disk()
                .map(|disk| disk.prune(installed, CAP_BYTES, dry_run))
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "compile_cache_prune_tests.rs"]
mod tests;
