//! Locating external executables (yt-dlp, ffmpeg, ffprobe, unrar, 7z).
//!
//! Search order: explicit setting, then the managed store, then the vendor directories, then
//! `PATH`. The vendor directories let a user drop every helper binary into one folder next to
//! the executable instead of installing them system-wide.
//!
//! The managed stage is filled by `rd-tools` (RD-102-02), which verifies, installs and
//! activates tool versions of its own. It sits *behind* the explicit setting on purpose: a
//! path somebody typed is a decision, and a managed download must never quietly replace it.
//! It sits *in front of* the vendor folders because a version this installation verified and
//! activated is a stronger statement than whatever happens to lie in a folder.
//!
//! `rd-core` must not depend on `rd-tools` — everything depends on `rd-core`, and the tool
//! store needs HTTP and signature verification. So the managed stage is a registered
//! resolver: `rd-tools` installs one at startup, and until it does the stage simply is not
//! there and the old order applies unchanged.

use std::{
    any::Any,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Folder name looked up next to the executable and inside the data directory.
pub const VENDOR_DIR_NAME: &str = "vendor";

static DATA_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Registers the data directory whose `vendor/` folder is searched. Called once at startup;
/// later calls are ignored.
pub fn set_data_directory(directory: impl Into<PathBuf>) {
    let _ = DATA_DIRECTORY.set(directory.into());
}

/// The registered data directory, i.e. where the database and its sidecar files live.
/// `None` in tests and tools that never called [`set_data_directory`].
#[must_use]
pub fn data_directory() -> Option<&'static Path> {
    DATA_DIRECTORY.get().map(PathBuf::as_path)
}

/// Where a tool was found.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    /// An absolute path configured in the settings.
    Explicit,
    /// A version this installation downloaded, verified and activated itself.
    Managed,
    /// One of the vendor directories.
    Vendor,
    /// A `PATH` entry.
    Path,
}

/// A located executable and where it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTool {
    pub path: PathBuf,
    pub source: ToolSource,
}

/// The directories searched before `PATH`, in order: the configured vendor folder,
/// `<exe dir>/vendor`, `<data dir>/vendor`, and finally the program folder itself.
///
/// The program folder is included because dropping the helper binaries next to the
/// executable is what people actually do with a portable app, and those binaries are not on
/// `PATH`. Non-existing entries are kept — probing a missing folder is cheap and keeps the
/// order stable for diagnostics. Duplicates are dropped even when not adjacent, which happens
/// whenever the configured folder is one of the built-in ones.
#[must_use]
pub fn vendor_directories(configured: Option<&str>) -> Vec<PathBuf> {
    let program_directory = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let candidates = [
        configured
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        program_directory
            .as_ref()
            .map(|parent| parent.join(VENDOR_DIR_NAME)),
        DATA_DIRECTORY.get().map(|data| data.join(VENDOR_DIR_NAME)),
        program_directory,
    ];
    let mut directories: Vec<PathBuf> = Vec::new();
    for directory in candidates.into_iter().flatten() {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

/// The active managed version of one tool, as the registered resolver reports it.
pub struct ManagedTool {
    /// The executable inside the managed store.
    pub path: PathBuf,
    /// The version that path belongs to, for status displays and diagnostics.
    pub version: String,
    /// Hold this for as long as the binary is used; the store will not delete a version
    /// while a lease on it is alive.
    pub lease: ToolLease,
}

/// An opaque, cloneable handle that marks one managed tool version as in use.
///
/// Type-erased on purpose. The counting belongs to the store in `rd-tools`, and naming the
/// guard here would mean `rd-core` — which everything else depends on — had to depend on the
/// tool store. A runner only ever needs to keep the value alive, never to look inside it.
#[derive(Clone)]
// The value exists to be *held* and then dropped, never read: that is the whole mechanism,
// and it is precisely the shape `dead_code` cannot see.
pub struct ToolLease(#[allow(dead_code)] Arc<dyn Any + Send + Sync>);

impl ToolLease {
    /// Wraps a store-owned guard. Called by `rd-tools`; nothing else has a guard to pass.
    #[must_use]
    pub fn new(guard: Arc<dyn Any + Send + Sync>) -> Self {
        Self(guard)
    }
}

impl std::fmt::Debug for ToolLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ToolLease")
    }
}

/// The managed stage of the lookup, implemented by `rd-tools`.
pub trait ManagedToolResolver: Send + Sync {
    /// The active managed binary for `name`, or `None` when this installation manages no
    /// version of it.
    fn resolve(&self, name: &str) -> Option<ManagedTool>;
}

static MANAGED_RESOLVER: OnceLock<Arc<dyn ManagedToolResolver>> = OnceLock::new();

/// Registers the managed stage. Called once at startup; later calls are ignored, the same
/// way [`set_data_directory`] is, so no part of the process can swap the tool store out from
/// under a running job.
pub fn set_managed_tool_resolver(resolver: Arc<dyn ManagedToolResolver>) {
    let _ = MANAGED_RESOLVER.set(resolver);
}

/// The active managed binary for `name`, when a resolver is registered and manages it.
#[must_use]
pub fn managed_tool(name: &str) -> Option<ManagedTool> {
    MANAGED_RESOLVER.get()?.resolve(name)
}

/// Resolves `name` against the explicit setting, the managed store, the vendor directories
/// and `PATH`.
///
/// An explicit path is authoritative: when it does not point at a file the lookup fails
/// instead of silently falling back, so a typo surfaces as "tool missing" rather than as a
/// different binary being used.
///
/// The lease returned by the managed stage is dropped immediately here, which is right for a
/// status query and wrong for a job. A caller that is about to *run* the binary wants
/// [`locate_tool_leased`] instead.
#[must_use]
pub fn locate_tool(
    explicit: Option<&str>,
    vendor: Option<&str>,
    name: &str,
) -> Option<ResolvedTool> {
    locate_tool_leased(explicit, vendor, name).map(|(tool, _)| tool)
}

/// [`locate_tool`], keeping the lease when the managed store answered.
///
/// Hold the returned lease for as long as the binary runs. Activation of another version
/// takes effect immediately for jobs that start afterwards, while a version somebody is
/// still executing stays on disk until the last lease on it is dropped — which is what makes
/// "running jobs keep their tool version" true without a per-job column anywhere.
#[must_use]
pub fn locate_tool_leased(
    explicit: Option<&str>,
    vendor: Option<&str>,
    name: &str,
) -> Option<(ResolvedTool, Option<ToolLease>)> {
    if let Some(explicit) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        return path.is_file().then(|| {
            (
                ResolvedTool {
                    path,
                    source: ToolSource::Explicit,
                },
                None,
            )
        });
    }
    if let Some(managed) = managed_tool(name) {
        return Some((
            ResolvedTool {
                path: managed.path,
                source: ToolSource::Managed,
            },
            Some(managed.lease),
        ));
    }
    for directory in vendor_directories(vendor) {
        if let Some(path) = executable_in(&directory, name) {
            return Some((
                ResolvedTool {
                    path,
                    source: ToolSource::Vendor,
                },
                None,
            ));
        }
    }
    let path_var = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_var) {
        if let Some(path) = executable_in(&directory, name) {
            return Some((
                ResolvedTool {
                    path,
                    source: ToolSource::Path,
                },
                None,
            ));
        }
    }
    None
}

/// Managed external tools (part of the `service.settings` blob, keys prefixed
/// `managed_tools_`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ManagedToolSettings {
    /// Whether this installation may download, verify and activate tool versions itself.
    /// Off by default: fetching executables is not something to start doing unasked.
    pub managed_tools_enabled: bool,
    /// `https://` URL of the signed tool manifest. `None` = use only the manifest compiled
    /// into this build, which is the offline-safe default.
    pub managed_tools_manifest_url: Option<String>,
    /// Tools whose compatibility verdict is reported but not enforced (RD-102-03).
    ///
    /// The explicit half of "override only explicit and auditable": naming a tool here does
    /// not hide the warning, it only stops the block. Every evaluation that skipped a block
    /// leaves a `tracing` record naming the tool and the rule.
    pub tool_compatibility_overrides: Vec<String>,
}

/// `name` (or `name.exe`) inside `directory`, when it exists.
#[must_use]
pub fn executable_in(directory: &Path, name: &str) -> Option<PathBuf> {
    for candidate in [name.to_owned(), format!("{name}.exe")] {
        let path = directory.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ManagedTool, ManagedToolResolver, ToolLease, ToolSource, locate_tool,
        set_managed_tool_resolver, vendor_directories,
    };

    #[test]
    fn explicit_path_does_not_fall_back() {
        assert!(locate_tool(Some("/definitely/not/here"), None, "yt-dlp").is_none());
    }

    #[test]
    fn blank_explicit_path_is_ignored() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tool = directory.path().join("rd-test-tool");
        std::fs::write(&tool, b"#!/bin/sh\n").expect("write tool");
        let vendor = directory.path().to_string_lossy().into_owned();
        let found = locate_tool(Some("   "), Some(&vendor), "rd-test-tool").expect("resolved");
        assert_eq!(found.source, ToolSource::Vendor);
        assert_eq!(found.path, tool);
    }

    /// The managed stage sits between the explicit setting and the vendor folders, and an
    /// explicit path still wins over it: a managed download must never quietly replace a
    /// decision somebody typed.
    ///
    /// One test rather than three, because the resolver is process-global and set once.
    #[test]
    fn the_managed_stage_sits_between_the_explicit_setting_and_the_vendor_folders() {
        struct OnlyOneTool(std::path::PathBuf);
        impl ManagedToolResolver for OnlyOneTool {
            fn resolve(&self, name: &str) -> Option<ManagedTool> {
                (name == "rd-managed-tool").then(|| ManagedTool {
                    path: self.0.clone(),
                    version: "1.2.3".to_owned(),
                    lease: ToolLease::new(Arc::new(())),
                })
            }
        }

        let directory = tempfile::tempdir().expect("tempdir");
        let managed = directory.path().join("managed-binary");
        std::fs::write(&managed, b"#!/bin/sh\n").expect("write managed");
        let vendored = directory.path().join("rd-managed-tool");
        std::fs::write(&vendored, b"#!/bin/sh\n").expect("write vendored");
        let vendor = directory.path().to_string_lossy().into_owned();
        set_managed_tool_resolver(Arc::new(OnlyOneTool(managed.clone())));

        let found = locate_tool(None, Some(&vendor), "rd-managed-tool").expect("resolved");
        assert_eq!(found.source, ToolSource::Managed);
        assert_eq!(found.path, managed);

        let explicit = locate_tool(
            Some(&vendored.to_string_lossy()),
            Some(&vendor),
            "rd-managed-tool",
        )
        .expect("resolved");
        assert_eq!(explicit.source, ToolSource::Explicit);
        assert_eq!(explicit.path, vendored);

        // A tool the resolver does not manage falls through to the folders exactly as before.
        let unmanaged = directory.path().join("rd-unmanaged-tool");
        std::fs::write(&unmanaged, b"#!/bin/sh\n").expect("write unmanaged");
        let other = locate_tool(None, Some(&vendor), "rd-unmanaged-tool").expect("resolved");
        assert_eq!(other.source, ToolSource::Vendor);
        assert_eq!(other.path, unmanaged);
    }

    #[test]
    fn configured_vendor_directory_comes_first_and_is_listed_once() {
        let directories = vendor_directories(Some("/opt/rd-vendor"));
        assert_eq!(
            directories.first().expect("configured entry").as_os_str(),
            "/opt/rd-vendor"
        );
        let unique: std::collections::HashSet<_> = directories.iter().collect();
        assert_eq!(unique.len(), directories.len(), "{directories:?}");
    }
}
