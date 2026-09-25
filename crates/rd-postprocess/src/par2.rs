use std::path::{Path, PathBuf};

use anyhow::Context;

/// Summary of verification and optional repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Par2Report {
    pub intact_files: usize,
    pub damaged_files: usize,
    pub missing_files: usize,
    pub blocks_needed: u32,
    pub repaired: bool,
    /// Recovery blocks the set itself still offers, for deciding how much more to fetch.
    pub blocks_available: u32,
}

/// Why PAR2 verification or repair did not succeed.
///
/// A bare `anyhow::Error` made the three cases indistinguishable, and they call for opposite
/// reactions (RD-104-04): an index nobody can parse says nothing about the payload and the
/// next member of the same set should be tried, while "not enough blocks" and "repair failed"
/// are verdicts about the payload itself.
#[derive(Debug)]
pub enum Par2Error {
    /// The index could not be parsed. The recovery data is broken, not necessarily the data.
    IndexUnreadable(anyhow::Error),
    /// Verification ran, and the recovery set is too small to close the gap.
    NotEnoughBlocks { needed: u32, available: u32 },
    /// Repair ran and did not succeed.
    RepairFailed(String),
    /// Anything else (I/O, a panicking blocking task).
    Other(anyhow::Error),
}

impl std::fmt::Display for Par2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IndexUnreadable(error) => write!(f, "PAR2 index could not be read: {error:#}"),
            Self::NotEnoughBlocks { needed, available } => write!(
                f,
                "PAR2 repair needs {needed} blocks but only {available} are available"
            ),
            Self::RepairFailed(message) => write!(f, "PAR2 repair failed: {message}"),
            Self::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for Par2Error {}

impl From<anyhow::Error> for Par2Error {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

impl Par2Error {
    /// Whether another member of the same set is worth trying instead.
    #[must_use]
    pub const fn is_index_unreadable(&self) -> bool {
        matches!(self, Self::IndexUnreadable(_))
    }
}

/// Runs CPU- and disk-heavy PAR2 work outside the async runtime.
pub async fn verify_and_repair(
    index: PathBuf,
    directory: PathBuf,
) -> Result<Par2Report, Par2Error> {
    tokio::task::spawn_blocking(move || {
        let file_set = rust_par2::parse(&index)
            .with_context(|| format!("parse PAR2 index {}", index.display()))
            .map_err(Par2Error::IndexUnreadable)?;
        let verification = rust_par2::verify(&file_set, &directory);
        let mut report = Par2Report {
            intact_files: verification.intact.len(),
            damaged_files: verification.damaged.len(),
            missing_files: verification.missing.len(),
            blocks_needed: verification.blocks_needed(),
            repaired: false,
            blocks_available: verification.recovery_blocks_available,
        };
        if verification.all_correct() {
            return Ok(report);
        }
        if !verification.repair_possible {
            return Err(Par2Error::NotEnoughBlocks {
                needed: verification.blocks_needed(),
                available: verification.recovery_blocks_available,
            });
        }
        let repair = rust_par2::repair_from_verify(&file_set, &directory, &verification)
            .context("PAR2 repair")?;
        if !repair.success {
            return Err(Par2Error::RepairFailed(repair.message));
        }
        report.repaired = true;
        Ok(report)
    })
    .await
    .map_err(|error| Par2Error::Other(error.into()))?
}

/// Verifies a package against the first member of `candidates` that can be read at all.
///
/// SABnzbd's `promote_par2` does the same thing from the other end: "in case of a broken par2
/// or missing par2, move another of the same set to the top". Every volume of a set carries
/// the same file descriptions, so a `.vol…` file answers the question the main index would
/// have answered — a corrupt index is a reason to ask a sibling, not to give up on a package
/// whose payload may be perfectly intact (RD-104-04).
///
/// Only an unreadable index moves on. "Not enough blocks" and "repair failed" are answers
/// about the payload, and asking a sibling would produce the same answer more slowly.
pub async fn verify_set(candidates: &[PathBuf], directory: &Path) -> Result<Par2Report, Par2Error> {
    let mut last = Par2Error::Other(anyhow::anyhow!("no PAR2 index to verify"));
    for candidate in candidates {
        match verify_and_repair(candidate.clone(), directory.to_owned()).await {
            Ok(report) => return Ok(report),
            Err(error) if error.is_index_unreadable() => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

/// The naming rules of a PAR2 set, shared with the queue.
///
/// They live in `rd-core` rather than here because the decision to hold a recovery volume back
/// is taken when the NZB is queued (RD-107-04), long before this crate ever sees a file, and
/// `rd-db` cannot depend on the post-processing crate. Re-exported so the PAR2 stage keeps
/// asking one module about PAR2 names.
pub use rd_core::{is_par2_index, is_par2_volume, par2_volume_belongs_to, par2_volume_blocks};

/// The recovery blocks the volume at `path` announces in its name; `None` when it says none.
///
/// The path counterpart of [`par2_volume_blocks`], for the callers that hold paths rather than
/// names. Deliberately not read from the file: the point of the number is to choose which
/// volumes to *fetch*, and those are exactly the ones not on disk yet.
#[must_use]
pub fn volume_blocks(path: &std::path::Path) -> Option<u32> {
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(par2_volume_blocks)
}

/// Main PAR2 index: `.par2` without `.vol`, or – for obfuscated names – a file starting
/// with the PAR2 packet magic whose name carries no volume marker.
#[must_use]
pub fn is_main_par2(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    if name.contains(".vol") {
        return false;
    }
    name.ends_with(".par2") || (!name.contains('.') && has_par2_magic(path))
}

/// The header check, shared with the Usenet transport through `rd-files` (RD-108-23).
pub use rd_files::has_par2_magic;

/// Every file of the recovery set belonging to one main index.
///
/// A PAR2 set is `release.par2` alongside `release.vol000+01.par2`, `release.vol001+02.par2`
/// and so on: the same stem, a `.vol` marker, the same extension. Matched by name rather than
/// by reading the files, because the set has to stay identifiable after a repair has already
/// rewritten what it protects.
///
/// An obfuscated index — no extension at all, recognised by its packet magic — is returned on
/// its own. Such a set carries random names throughout, so there is nothing to match on, and
/// deleting a file because it happens to sit in the same folder is not a guess worth making.
#[must_use]
pub fn par2_set(index: &std::path::Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut set = vec![index.to_path_buf()];
    let Some(name) = index.file_name().and_then(|value| value.to_str()) else {
        return set;
    };
    let lowercase = name.to_ascii_lowercase();
    let Some(stem) = lowercase.strip_suffix(".par2") else {
        // Obfuscated: nothing reliable to match siblings on.
        return set;
    };
    let prefix = format!("{stem}.vol");
    for candidate in candidates {
        if candidate == index {
            continue;
        }
        let Some(other) = candidate.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let other = other.to_ascii_lowercase();
        if other.starts_with(&prefix) && other.ends_with(".par2") {
            set.push(candidate.clone());
        }
    }
    set.sort();
    set.dedup();
    set
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Par2Error, par2_set, verify_set};

    /// A corrupt main index has to send the search on to the rest of its set.
    ///
    /// SABnzbd's `promote_par2` does the same from the other end. The proof that the sibling
    /// was really tried is in the error: `verify_set` reports the *last* candidate it could
    /// not read, so a message naming the volume means the main index was not the end of it.
    #[tokio::test]
    async fn a_corrupt_index_sends_the_search_on_to_the_rest_of_the_set() {
        let temp = tempfile::tempdir().expect("tempdir");
        let index = temp.path().join("release.par2");
        let volume = temp.path().join("release.vol000+01.par2");
        std::fs::write(&index, b"this is not a PAR2 packet").expect("index");
        std::fs::write(&volume, b"neither is this one").expect("volume");
        let candidates = par2_set(&index, &[index.clone(), volume.clone()]);
        assert_eq!(candidates.len(), 2);

        let error = verify_set(&candidates, temp.path())
            .await
            .expect_err("nothing in this set can be parsed");
        assert!(
            matches!(error, Par2Error::IndexUnreadable(_)),
            "an unreadable index is not a verdict about the payload: {error}"
        );
        assert!(
            error.to_string().contains("release.vol000+01.par2"),
            "the volume was never tried: {error}"
        );
    }

    #[tokio::test]
    async fn a_set_with_nothing_in_it_says_so_rather_than_claiming_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let error = verify_set(&[], temp.path())
            .await
            .expect_err("there is nothing to verify");
        assert!(matches!(error, Par2Error::Other(_)), "{error}");
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from("/pkg").join(name))
            .collect()
    }

    #[test]
    fn a_set_is_the_index_and_its_volumes() {
        let files = paths(&[
            "release.par2",
            "release.vol000+01.par2",
            "release.vol001+02.par2",
            "release.rar",
            "release.r00",
        ]);
        let set = par2_set(&PathBuf::from("/pkg/release.par2"), &files);
        assert_eq!(
            set,
            paths(&[
                "release.par2",
                "release.vol000+01.par2",
                "release.vol001+02.par2"
            ])
        );
    }

    #[test]
    fn another_release_in_the_same_folder_is_left_alone() {
        // Two releases unpacked side by side is ordinary. Deleting the wrong one's recovery
        // data would be silent and unrecoverable.
        let files = paths(&[
            "one.par2",
            "one.vol000+01.par2",
            "two.par2",
            "two.vol000+01.par2",
        ]);
        let set = par2_set(&PathBuf::from("/pkg/one.par2"), &files);
        assert_eq!(set, paths(&["one.par2", "one.vol000+01.par2"]));
    }

    #[test]
    fn matching_ignores_case_the_way_the_names_arrive() {
        let files = paths(&["Release.PAR2", "Release.Vol000+01.PAR2"]);
        let set = par2_set(&PathBuf::from("/pkg/Release.PAR2"), &files);
        assert_eq!(set.len(), 2, "{set:?}");
    }

    #[test]
    fn an_obfuscated_index_is_returned_on_its_own() {
        // An obfuscated set has random names throughout; there is nothing to match siblings
        // on, so nothing else is claimed.
        let files = paths(&["a1b2c3", "d4e5f6", "release.rar"]);
        let set = par2_set(&PathBuf::from("/pkg/a1b2c3"), &files);
        assert_eq!(set, paths(&["a1b2c3"]));
    }

    #[test]
    fn a_file_that_merely_starts_the_same_is_not_a_volume() {
        let files = paths(&["release.par2", "release.volume-notes.txt", "release2.par2"]);
        let set = par2_set(&PathBuf::from("/pkg/release.par2"), &files);
        assert_eq!(set, paths(&["release.par2"]));
    }
}
