//! Where a set of archives is unpacked to: the package folder, a folder of its own, or the
//! folder its parent archive went into (RD-170-16).

use std::path::{Path, PathBuf};

use rd_postprocess::ArchiveSet;

/// Where a set is unpacked to (RD-170-16).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnpackTarget {
    /// Straight into the package folder, merged with what is there — the default.
    Package,
    /// Into a folder of its own below the package folder, named after the archive
    /// (`Film.part1.rar` → `Film/`, a second set named `Film` → `Film (1)/`). The archives
    /// themselves stay where they are.
    OwnFolder,
    /// A nested pass under [`Self::OwnFolder`]: an archive that came out of an archive is
    /// unpacked inside the folder its parent went into, never beside it in the package root.
    EnclosingFolder,
}

impl UnpackTarget {
    /// The directory each of `sets` is unpacked into, in the same order.
    ///
    /// Computed for all sets at once, because a folder of its own has to be one no other set
    /// of the package claims: `Film.zip` and `Film.rar` go to `Film` and `Film (1)` (RD-190-06).
    /// The sets come sorted from `group_archive_sets`, so the same package hands out the same
    /// names on every run.
    pub(crate) fn destinations(self, directory: &Path, sets: &[ArchiveSet]) -> Vec<PathBuf> {
        match self {
            Self::Package => vec![directory.to_owned(); sets.len()],
            Self::OwnFolder => {
                let bases: Vec<&str> = sets.iter().map(|set| set.base.as_str()).collect();
                rd_files::extraction_subfolders(directory, &bases)
            }
            Self::EnclosingFolder => {
                let enclosing: Vec<Option<PathBuf>> = sets
                    .iter()
                    .map(|set| {
                        let relative = set.first().strip_prefix(directory).ok()?;
                        let mut components = relative.components();
                        let top = components.next()?;
                        // Only a set below a folder has one to stay in; the rest has a name.
                        components.next()?;
                        Some(directory.join(top))
                    })
                    .collect();
                let bases: Vec<&str> = sets
                    .iter()
                    .zip(&enclosing)
                    .filter(|(_, folder)| folder.is_none())
                    .map(|(set, _)| set.base.as_str())
                    .collect();
                let mut own = rd_files::extraction_subfolders(directory, &bases).into_iter();
                enclosing
                    .into_iter()
                    .map(|folder| {
                        folder
                            .or_else(|| own.next())
                            .unwrap_or_else(|| directory.to_owned())
                    })
                    .collect()
            }
        }
    }
}
