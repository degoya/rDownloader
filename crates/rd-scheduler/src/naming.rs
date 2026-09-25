//! The package name a resolver's file name supplies (RD-109-45).
//!
//! A single link whose address carries no path segment gives intake nothing to name its
//! package after, so `rd_collector::group_links` falls back to the host and the package is
//! called `1fichier.com`. The real name exists — the link page states it — but it is only
//! learned when the resolver runs, long after the package row was written.
//!
//! **The folder is named, not renamed.** This runs from the worker while the download is still
//! `Resolving`: the destination directory is created at `worker.rs`'s `StorageRoot::create`,
//! which happens *after* the resolver has spoken, so for a fresh transfer the folder is simply
//! created under the right name and no data moves at all. Only an attempt that already got as
//! far as creating the folder leaves something behind, and that is carried over by the existing
//! two-phase move (`rename_package_directory` + `relocate_package`, RD-106-13) rather than by
//! anything new.

use std::path::PathBuf;

use anyhow::Result;
use rd_core::DownloadFile;
use url::Url;

use crate::SchedulerHandle;

/// The name intake substitutes when neither the source nor the address offers one
/// (`collector_enqueue.rs`). A package called `download` says less than the hoster does.
const PLACEHOLDER: &str = "download";

/// Shortest name worth having — the floor `rd_collector::common_stem` already applies, so both
/// ways of naming a package agree on what is too short to mean anything.
const MIN_NAME_CHARS: usize = 3;

/// The name a newly learned file name gives the package, or `None` when the package keeps the
/// one it has.
///
/// Renaming is confined to a package that still carries the automatic fallback: its name *is*
/// the host of its link, which is the one string `group_links` produces when it knows nothing
/// else. A package anybody named — by hand, from a container, from a common stem — differs from
/// that string and is left alone, which is what keeps this out of `auto_named = 0` territory
/// without a column that says so.
pub(crate) fn resolved_package_name(
    source: &Url,
    package_name: &str,
    file_name: &str,
) -> Option<String> {
    let host = source.host_str()?.trim_start_matches("www.");
    if host.is_empty() || !package_name.trim().eq_ignore_ascii_case(host) {
        return None;
    }
    let release = rd_files::package_name_from_file_name(file_name.trim());
    let release = release.trim();
    if release.chars().count() < MIN_NAME_CHARS
        || release.eq_ignore_ascii_case(PLACEHOLDER)
        || release.eq_ignore_ascii_case(host)
    {
        return None;
    }
    // Sanitizing can still empty a name out — `...` trims to nothing — and
    // `sanitize_file_name` then substitutes its own placeholder. Checked again rather than
    // assumed, because that placeholder as a package name is worse than the hoster.
    let sanitized = rd_files::sanitize_file_name(release);
    (!sanitized.eq_ignore_ascii_case(host) && !sanitized.eq_ignore_ascii_case(PLACEHOLDER))
        .then_some(sanitized)
}

impl SchedulerHandle {
    /// Gives a package that is still named after its hoster the release name the resolver just
    /// learned, and points its folder at the same name.
    ///
    /// Silent and cheap in every case but the one it exists for: a package with a name of its
    /// own, a package holding more than this one file, a resume that already has bytes on disk,
    /// and a target folder that is already taken all leave the package exactly as it was.
    ///
    /// **A taken folder stops the rename.** Every other automatic path in this codebase reaches
    /// for `rd_files::collision_free_path` and stores `name (1)`. That is right for a file
    /// landing beside another; it is wrong for a folder, because an existing folder of that name
    /// holds somebody else's package, and neither merging into it nor inventing `release (1)` is
    /// better than the hoster name the package already has.
    pub async fn adopt_resolved_package_name(
        &self,
        file: &DownloadFile,
        file_name: &str,
    ) -> Result<()> {
        // A resume has a staging file in the folder this would move, and moving it out from
        // under a transfer that is about to continue costs the resume point. Only a transfer
        // that has committed nothing can be renamed for free.
        if file.committed_bytes.get() > 0 {
            return Ok(());
        }
        let packages = self.database.list_packages().await?;
        let Some(package) = packages
            .iter()
            .find(|package| package.id == file.package_id)
        else {
            return Ok(());
        };
        if package.destination.is_empty() {
            return Ok(());
        }
        let Some(name) = resolved_package_name(&file.source, &package.name, file_name) else {
            return Ok(());
        };
        // One file's name is a statement about the package only while it is the only file in
        // it. A second link makes the package a set, and the set is named by grouping.
        let siblings = self
            .database
            .list_downloads()
            .await?
            .into_iter()
            .filter(|other| other.package_id == package.id)
            .count();
        if siblings != 1 {
            return Ok(());
        }
        let current = PathBuf::from(&package.destination);
        let Some(target) = rd_files::renamed_package_directory(&current, &name) else {
            return Ok(());
        };
        let destination = target.to_string_lossy().into_owned();
        if destination != package.destination {
            let taken = tokio::fs::try_exists(&target).await.unwrap_or(true)
                || packages
                    .iter()
                    .any(|other| other.id != package.id && other.destination == destination);
            if taken {
                tracing::info!(
                    package_id = %package.id,
                    %name,
                    "the package keeps its hoster name: that folder is already taken"
                );
                return Ok(());
            }
        }
        let package_id = package.id;
        self.database
            .rename_package_directory(package_id, name, destination)
            .await?;
        // Phase two of the same two-phase protocol a folder rename uses. In the case this
        // exists for there is nothing to carry over — the folder was never created — and
        // leaving `previous_destination` set would record an outstanding move over a directory
        // that never held anything. A crash between the two writes is survivable either way:
        // the next completed attempt runs `relocate_package`, which finds nothing under the old
        // path and clears the record itself.
        if tokio::fs::try_exists(&current).await.unwrap_or(true) {
            self.relocate_package(package_id).await?;
        } else {
            self.database
                .clear_package_previous_destination(package_id)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::resolved_package_name;

    fn name(url: &str, package: &str, file: &str) -> Option<String> {
        resolved_package_name(&url.parse().expect("url"), package, file)
    }

    #[test]
    fn a_hoster_named_package_takes_the_release_behind_the_file_name() {
        assert_eq!(
            name(
                "https://1fichier.com/?8x6wertoi51r8vptrojn",
                "1fichier.com",
                "outlander.s08e01.german.bdrip.x264-intention.rar"
            )
            .as_deref(),
            Some("outlander.s08e01.german.bdrip.x264-intention")
        );
    }

    #[test]
    fn the_www_prefix_does_not_hide_the_fallback() {
        assert_eq!(
            name("https://www.example.com/x", "example.com", "Movie.2024.mkv").as_deref(),
            Some("Movie.2024")
        );
    }

    #[test]
    fn a_name_of_its_own_is_never_overwritten() {
        assert_eq!(
            name(
                "https://1fichier.com/?x",
                "Outlander Staffel 8",
                "outlander.s08e01.rar"
            ),
            None
        );
    }

    #[test]
    fn the_intake_placeholder_is_no_better_than_the_hoster() {
        assert_eq!(
            name("https://1fichier.com/?x", "1fichier.com", "download.bin"),
            None
        );
        assert_eq!(
            name("https://1fichier.com/?x", "1fichier.com", "a.mp4"),
            None
        );
    }

    #[test]
    fn a_name_that_sanitizes_away_to_the_placeholder_changes_nothing() {
        assert_eq!(name("https://1fichier.com/?x", "1fichier.com", "..."), None);
    }

    #[test]
    fn a_name_that_sanitizes_to_the_host_changes_nothing() {
        assert_eq!(
            name("https://1fichier.com/?x", "1fichier.com", "1fichier.com"),
            None
        );
    }
}
