//! Which refused version directories this start has already warned about (RD-1291-03).
//!
//! The installed packages are re-verified on every listing, locale bundle and load, several
//! dozen times per start, and each pass used to warn again for every version that does not
//! verify: a tester's diagnostic bundle held 200 warnings from one start, all of them about
//! three old versions, and every warning that mattered had been pushed out of it. The start's
//! sweep (`sweep.rs`) reports each refused version once, summarised per plugin; every later
//! pass over the same directory logs at debug level.

use std::{
    collections::HashSet,
    fmt::Display,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// The version directories whose refusal was already reported, shared by every clone of the
/// installer.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReportedRefusals(Arc<Mutex<HashSet<PathBuf>>>);

impl ReportedRefusals {
    /// Records `path`; `true` the first time it is named. A poisoned lock reports again rather
    /// than never.
    pub(crate) fn first(&self, path: &Path) -> bool {
        match self.0.lock() {
            Ok(mut reported) => reported.insert(path.to_path_buf()),
            Err(_) => true,
        }
    }

    /// Logs one skipped version: a warning the first time this start meets it, debug after.
    pub(super) fn skip(&self, path: &Path, error: &impl Display, message: &str) {
        if self.first(path) {
            tracing::warn!(path = %path.display(), error = %error, "{message}");
        } else {
            tracing::debug!(path = %path.display(), error = %error, "{message}");
        }
    }
}
