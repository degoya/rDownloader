//! SABnzbd-style post-processing model: per-package level, package lifecycle and the
//! stage/progress a running pipeline reports.

use std::{
    fmt,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Cumulative post-processing level (`Repair` ⊂ `Unpack` ⊂ `Delete`), like SABnzbd's
/// `+R` / `+RU` / `+RUD` job options.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessLevel {
    /// Nothing runs automatically (extraction stays available manually).
    None,
    /// PAR2 verification/repair only.
    Repair,
    /// Repair, then unpack archives into the package folder.
    #[default]
    Unpack,
    /// Repair, unpack and delete the archive volumes afterwards.
    Delete,
}

impl PostprocessLevel {
    #[must_use]
    pub fn repairs(self) -> bool {
        self >= Self::Repair
    }

    #[must_use]
    pub fn unpacks(self) -> bool {
        self >= Self::Unpack
    }

    #[must_use]
    pub fn deletes(self) -> bool {
        self == Self::Delete
    }
}

/// Lifecycle of a download package as a whole.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    #[default]
    Queued,
    Downloading,
    Postprocessing,
    Completed,
    /// Post-processing failed (repair or unpack); downloads themselves may be complete.
    Failed,
}

impl fmt::Display for PackageState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        f.write_str(text.trim_matches('"'))
    }
}

impl FromStr for PackageState {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{value}\""))
    }
}

/// Pipeline stage a package is currently in while post-processing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessStage {
    Repairing,
    Verifying,
    Extracting,
    DeletingArchives,
    DeletingPar2,
    Cleaning,
    /// Joining a livestream recording's segments into one container (RD-080-09).
    Remuxing,
    /// A step contributed by a post-processing plugin (RD-090-16).
    PluginStep,
    Script,
    Uploading,
}

impl fmt::Display for PostprocessStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        f.write_str(text.trim_matches('"'))
    }
}

impl FromStr for PostprocessStage {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{value}\""))
    }
}

/// Whether a file name is the main index of a PAR2 set (`release.par2`).
///
/// Name-based on purpose: the decision has to be made when an NZB is queued, long before any
/// of its files exist on disk to be read. [`crate::is_par2_volume`] is the counterpart, and the
/// two together are what lets the recovery volumes be held back while the index still comes
/// down (RD-107-04). An obfuscated index carries no `.par2` extension at all and is therefore
/// neither: nothing is postponed for such a set, which is the safe answer.
#[must_use]
pub fn is_par2_index(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    name.ends_with(".par2") && !name.contains(".vol")
}

/// Whether a file name is a recovery volume of a PAR2 set (`release.vol000+01.par2`).
#[must_use]
pub fn is_par2_volume(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    name.ends_with(".par2") && name.contains(".vol")
}

/// The recovery blocks a volume announces in its own name.
///
/// `release.vol000+01.par2` carries one block, `release.vol031+16.par2` sixteen. Both the
/// par2cmdline spelling (`+`) and the QuickPar one (`-`) are accepted, as SABnzbd does: the
/// second number is the block count either way. `None` means the name does not say, and a
/// caller that has to pick volumes should then assume the smallest useful value rather than
/// treat the volume as worthless.
#[must_use]
pub fn par2_volume_blocks(file_name: &str) -> Option<u32> {
    let name = file_name.to_ascii_lowercase();
    let stem = name.strip_suffix(".par2")?;
    let (_, tail) = stem.rsplit_once(".vol")?;
    let separator = tail.find(['+', '-'])?;
    let (start, blocks) = tail.split_at(separator);
    if start.is_empty() || !start.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let blocks = &blocks[1..];
    if blocks.is_empty() || !blocks.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    blocks.parse().ok()
}

/// Whether `volume` is a recovery volume of the set whose main index is `index`.
///
/// The same rule [`crate::par2_volume_blocks`] relies on and `rd_postprocess::par2_set` applies
/// to paths: one stem, a `.vol` marker, the same extension.
#[must_use]
pub fn par2_volume_belongs_to(index: &str, volume: &str) -> bool {
    let index = index.to_ascii_lowercase();
    let Some(stem) = index.strip_suffix(".par2") else {
        return false;
    };
    let volume = volume.to_ascii_lowercase();
    volume.starts_with(&format!("{stem}.vol")) && volume.ends_with(".par2")
}

/// Persisted outcome of the unpack stage, kept after the pipeline finished so the UI can
/// show whether a package was extracted. `None` on the package means nothing was unpacked
/// (no archives, or the level does not unpack).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionResult {
    Success,
    Failed,
}

impl fmt::Display for ExtractionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        f.write_str(text.trim_matches('"'))
    }
}

impl FromStr for ExtractionResult {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{value}\""))
    }
}

/// Live status of a package's post-processing pipeline.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PostprocessStatus {
    pub stage: Option<PostprocessStage>,
    /// 0–100 for the current stage when known.
    pub percent: Option<u8>,
    /// File or script currently being processed.
    pub current: Option<String>,
}

/// Counter of packages currently post-processing; the scheduler stops dispatching new
/// files while it is above zero and `pause_during_postprocess` is enabled.
#[derive(Clone, Debug, Default)]
pub struct PostprocessHold(Arc<AtomicUsize>);

impl PostprocessHold {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether at least one package is being post-processed.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.0.load(Ordering::Acquire) > 0
    }

    /// Marks one running pipeline; released when the guard drops.
    #[must_use]
    pub fn acquire(&self) -> PostprocessHoldGuard {
        self.0.fetch_add(1, Ordering::AcqRel);
        PostprocessHoldGuard(Arc::clone(&self.0))
    }
}

/// Releases one hold on drop (also on panic/early return).
#[derive(Debug)]
pub struct PostprocessHoldGuard(Arc<AtomicUsize>);

impl Drop for PostprocessHoldGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::{PackageState, PostprocessHold, PostprocessLevel};

    #[test]
    fn levels_are_cumulative() {
        assert!(!PostprocessLevel::None.repairs());
        assert!(PostprocessLevel::Repair.repairs() && !PostprocessLevel::Repair.unpacks());
        assert!(PostprocessLevel::Unpack.unpacks() && !PostprocessLevel::Unpack.deletes());
        assert!(PostprocessLevel::Delete.deletes());
        assert_eq!(
            serde_json::to_string(&PostprocessLevel::Delete).expect("json"),
            "\"delete\""
        );
    }

    #[test]
    fn package_state_round_trips_through_text() {
        let state: PackageState = "postprocessing".parse().expect("parse");
        assert_eq!(state, PackageState::Postprocessing);
        assert_eq!(state.to_string(), "postprocessing");
    }

    #[test]
    fn a_volume_name_carries_its_own_block_count() {
        assert_eq!(super::par2_volume_blocks("release.vol000+01.par2"), Some(1));
        assert_eq!(
            super::par2_volume_blocks("release.vol031+16.PAR2"),
            Some(16)
        );
        // QuickPar writes the same thing with a dash; the second number still counts blocks.
        assert_eq!(
            super::par2_volume_blocks("ampnjl08.vol07-15.par2"),
            Some(15)
        );
        // The main index is not a volume and announces nothing.
        assert_eq!(super::par2_volume_blocks("release.par2"), None);
        assert_eq!(super::par2_volume_blocks("release.vol.par2"), None);
    }

    #[test]
    fn an_index_and_its_volumes_are_told_apart() {
        assert!(super::is_par2_index("release.par2"));
        assert!(!super::is_par2_index("release.vol000+01.par2"));
        assert!(super::is_par2_volume("release.vol000+01.par2"));
        assert!(!super::is_par2_volume("release.par2"));
        // An obfuscated index has no extension to go on and is neither.
        assert!(!super::is_par2_index("d41d8cd98f00b204"));
        assert!(!super::is_par2_volume("d41d8cd98f00b204"));
    }

    #[test]
    fn a_volume_belongs_only_to_its_own_set() {
        assert!(super::par2_volume_belongs_to(
            "release.par2",
            "release.vol000+01.par2"
        ));
        assert!(!super::par2_volume_belongs_to(
            "release.par2",
            "other.vol000+01.par2"
        ));
        assert!(!super::par2_volume_belongs_to(
            "release.par2",
            "release.vol000+01.par2.tmp"
        ));
    }

    #[test]
    fn hold_is_released_by_guard() {
        let hold = PostprocessHold::new();
        assert!(!hold.is_held());
        let guard = hold.acquire();
        assert!(hold.is_held());
        drop(guard);
        assert!(!hold.is_held());
    }
}
