//! Compiled plugin code kept on disk between starts (RD-130-06).
//!
//! Every start used to compile every installed component again: 34 s to the first answer with
//! 72 bundled plugins in a release build, about seven minutes in a debug one. Wasmtime's own
//! cache keeps the machine code instead. It keys an entry by the component's bytes, the engine
//! configuration and — in the directory name, `modules/wasmtime-<version>` — the Wasmtime
//! release, so a changed plugin and a Wasmtime update both compile anew by construction.
//!
//! What it does not do is check what it reads back. An entry is compressed machine code, and
//! a byte flipped inside the compressed payload can decompress into different instructions
//! that run with the service's own rights, not in the sandbox. Wasmtime only refuses what no
//! longer parses. So this crate keeps its own record: the SHA-256 of every entry the service
//! wrote itself, checked before the engine exists. An entry that does not match — damaged,
//! copied in from elsewhere, or simply not written here — is deleted, and the component is
//! compiled again. The worst a bad entry can cost is that compile.
//!
//! The record is a checksum, not a signature. Whoever can write the data directory can change
//! what the service trusts anyway — the database and the managed tools it executes live
//! there too — so it is not meant to stop them, only to make sure that what runs is what this
//! service compiled. The package itself is verified on every load exactly as before; the cache
//! sits behind that check, never in front of it.
//!
//! Nor does Wasmtime's cache ever forget what nothing uses any more (RD-1240-34): every plugin
//! update and every Wasmtime release left its entries behind, 263 MB of them on the owner's
//! installation for thirteen plugins. So the record also names, per entry, the components that
//! were being compiled when it appeared — the one that wrote it among them — and which Wasmtime
//! directory this build writes to; [`prune`] removes what belongs to no installed component or
//! to another Wasmtime, and the oldest entries beyond a size cap.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use wasmtime::{Cache, CacheConfig};

#[path = "compile_cache_prune.rs"]
pub(crate) mod prune;

use prune::{OWNERS_FILE, Owners, read_owners, write_owners};

/// The record of trusted entries, beside the directory Wasmtime owns.
const INDEX_FILE: &str = "integrity.json";
/// Wasmtime's own directory under the cache root.
const WASMTIME_DIRECTORY: &str = "wasmtime";
/// An entry's name is the unpadded URL-safe base64 of a SHA-256: 43 characters.
const ENTRY_NAME_LENGTH: usize = 43;

/// Wasmtime's cache with the integrity record around it.
pub(crate) struct CompileCache {
    modules: PathBuf,
    index_path: PathBuf,
    /// Relative entry path (`<version directory>/<entry>`) to the hex SHA-256 of its bytes.
    index: Mutex<BTreeMap<String, String>>,
    /// Which components each entry belongs to, beside the index; locked after it.
    owners_path: PathBuf,
    owners: Mutex<Owners>,
    /// The hex SHA-256 of every component being compiled right now, with how many compiles of it.
    in_flight: Mutex<BTreeMap<String, usize>>,
    cache: Cache,
}

/// One compile under way; ends with the value.
pub(crate) struct InFlight<'a> {
    cache: &'a CompileCache,
    component: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = self.cache.in_flight.lock()
            && let Some(count) = in_flight.get_mut(&self.component)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.remove(&self.component);
            }
        }
    }
}

impl CompileCache {
    /// Opens the cache under `directory`, deleting every entry the record does not vouch for.
    ///
    /// Runs before the engine is built, so no entry is read that has not been checked.
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        // Wasmtime refuses a relative cache directory, and the data directory may be one.
        let directory = std::path::absolute(directory)
            .with_context(|| format!("resolve plugin cache path {}", directory.display()))?;
        let wasmtime_directory = directory.join(WASMTIME_DIRECTORY);
        std::fs::create_dir_all(&wasmtime_directory).with_context(|| {
            format!(
                "create plugin cache directory {}",
                wasmtime_directory.display()
            )
        })?;
        let index_path = directory.join(INDEX_FILE);
        let modules = wasmtime_directory.join("modules");
        let recorded = read_index(&index_path);
        let mut kept = BTreeMap::new();
        let mut removed = 0_usize;
        for (key, path) in entries(&modules) {
            match (recorded.get(&key), file_digest(&path)) {
                (Some(expected), Some(actual)) if *expected == actual => {
                    kept.insert(key, actual);
                }
                _ => {
                    removed += 1;
                    if let Err(error) = std::fs::remove_file(&path) {
                        // Left in place it would be read back unchecked, so this is not a
                        // warning to shrug at: without the cache the service is only slower.
                        anyhow::bail!(
                            "could not remove unverified plugin cache entry {}: {error}",
                            path.display()
                        );
                    }
                }
            }
        }
        if removed > 0 {
            tracing::warn!(
                removed,
                "removed plugin cache entries this service did not record; they compile again"
            );
        }
        write_index(&index_path, &kept)?;
        let owners_path = directory.join(OWNERS_FILE);
        let mut owners = read_owners(&owners_path);
        owners.entries.retain(|key, _| kept.contains_key(key));
        if let Err(error) = write_owners(&owners_path, &owners) {
            tracing::warn!(%error, "could not record whose plugin cache entries are whose");
        }

        let mut config = CacheConfig::new();
        config
            .with_directory(wasmtime_directory)
            // Recompressing a used entry rewrites its bytes, which the record would then
            // refuse at the next start and cost a compile for nothing.
            .with_optimized_compression_usage_counter_threshold(u64::MAX)
            // Wasmtime's own cleanup, which runs when an entry is written, holds the same cap
            // `prune` does between two of its passes.
            .with_files_total_size_soft_limit(prune::CAP_BYTES);
        let cache = Cache::new(config)
            .map_err(|error| anyhow::anyhow!("configure the plugin compile cache: {error}"))?;
        Ok(Self {
            modules,
            index_path,
            index: Mutex::new(kept),
            owners_path,
            owners: Mutex::new(owners),
            in_flight: Mutex::new(BTreeMap::new()),
            cache,
        })
    }

    /// Marks `component` (its hex SHA-256) as being compiled until the value is dropped, which
    /// is after [`Self::record_new_entries`]: whatever a compile writes appears while its
    /// component is marked.
    pub(crate) fn begin(&self, component: &str) -> InFlight<'_> {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            *in_flight.entry(component.to_owned()).or_default() += 1;
        }
        InFlight {
            cache: self,
            component: component.to_owned(),
        }
    }

    /// The Wasmtime cache the engine is configured with.
    pub(crate) fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Records the entries a compile has just written, so the next start keeps them.
    ///
    /// Wasmtime writes its entry before `Component::from_binary` returns, so every file that
    /// is new at this point was written by this process. Only unrecorded files are hashed;
    /// after a start with nothing new this is one directory listing.
    ///
    /// A new entry belongs to the components being compiled now: the one that wrote it is
    /// among them, since it stays marked until it has recorded its own entry. With one compile
    /// at a time — a start compiles one plugin after the other — that is exactly it.
    pub(crate) fn record_new_entries(&self) {
        let Ok(mut index) = self.index.lock() else {
            return;
        };
        let compiling: std::collections::BTreeSet<String> = self
            .in_flight
            .lock()
            .map(|in_flight| in_flight.keys().cloned().collect())
            .unwrap_or_default();
        let mut new = Vec::new();
        for (key, path) in entries(&self.modules) {
            if index.contains_key(&key) {
                continue;
            }
            if let Some(digest) = file_digest(&path) {
                index.insert(key.clone(), digest);
                new.push(key);
            }
        }
        if new.is_empty() {
            return;
        }
        if let Err(error) = write_index(&self.index_path, &index) {
            // Not recorded means deleted and compiled again at the next start: slower, not
            // wrong.
            tracing::warn!(%error, "could not record the plugin compile cache");
        }
        let Ok(mut owners) = self.owners.lock() else {
            return;
        };
        for key in new {
            if let Some((directory, _)) = key.split_once('/') {
                owners.compiler_directory = Some(directory.to_owned());
            }
            owners.entries.insert(key, compiling.clone());
        }
        if let Err(error) = write_owners(&self.owners_path, &owners) {
            // An entry without owners goes at the next prune and compiles again once.
            tracing::warn!(%error, "could not record whose plugin cache entries are whose");
        }
    }
}

/// Every file in `modules/<version>/` whose name is an entry name, keyed by its relative path.
///
/// Anything else there — Wasmtime's usage statistics, half-written temporary files — is never
/// read as compiled code, so it is left to Wasmtime's own cleanup.
fn entries(modules: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let Ok(versions) = std::fs::read_dir(modules) else {
        return found;
    };
    for version in versions.flatten() {
        let Ok(files) = std::fs::read_dir(version.path()) else {
            continue;
        };
        let version_name = version.file_name().to_string_lossy().into_owned();
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if is_entry_name(&name) {
                found.push((format!("{version_name}/{name}"), file.path()));
            }
        }
    }
    found
}

fn is_entry_name(name: &str) -> bool {
    name.len() == ENTRY_NAME_LENGTH
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The hex SHA-256 of a regular file; `None` for anything else, which is then removed.
///
/// A symlink is not followed: an entry that points elsewhere is not one this service wrote.
fn file_digest(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(hex_digest(&bytes))
}

/// The hex SHA-256 of `bytes`: how an entry's content and a component are both named.
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lower-case hex of a digest.
pub(crate) fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The stored record; an unreadable one is an empty one, which only costs compiles.
fn read_index(path: &Path) -> BTreeMap<String, String> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, "plugin compile cache record is unreadable; starting it afresh");
        BTreeMap::new()
    })
}

/// Writes the record through a temporary file, so a crash leaves the old one or the new one.
fn write_index(path: &Path, index: &BTreeMap<String, String>) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(index)?)
        .with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

#[cfg(test)]
#[path = "compile_cache_tests.rs"]
mod tests;
