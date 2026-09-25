use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rd_files::{ArchiveKind, parse_archive_volume};

/// A complete archive set: every volume present, sorted, starting with the entry volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveSet {
    pub kind: ArchiveKind,
    pub base: String,
    /// Volumes in extraction order (index 0 = entry volume).
    pub volumes: Vec<PathBuf>,
}

impl ArchiveSet {
    /// Volume extraction starts from.
    #[must_use]
    pub fn first(&self) -> &Path {
        &self.volumes[0]
    }

    /// Whether the set consists of more than one volume.
    #[must_use]
    pub fn is_multipart(&self) -> bool {
        self.volumes.len() > 1
    }
}

/// Groups files into complete archive sets; incomplete sets (missing entry volume or gaps)
/// are omitted so nothing is extracted twice or from the middle.
///
/// Two names can claim the same slot: `Release.rar` and `Release.part01.rar` are both index 0 of
/// the same set. The winner used to be whichever path came first in `files` — a directory listing
/// order, so the extraction could start from a different volume between two runs. The numbered
/// name wins now, and a tie is broken by the file name, so the choice is the same every time
/// (RD-107-11).
#[must_use]
pub fn group_archive_sets(files: &[PathBuf]) -> Vec<ArchiveSet> {
    let mut groups: BTreeMap<(u8, String), BTreeMap<u32, Candidate>> = BTreeMap::new();
    for path in files {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(volume) = parse_archive_volume(name) else {
            continue;
        };
        let key = (kind_rank(volume.kind), volume.base.to_ascii_lowercase());
        let slot = groups.entry(key).or_default().entry(volume.index);
        let candidate = Candidate {
            path: path.clone(),
            is_first: volume.is_first,
            numbered: volume.numbered,
        };
        match slot {
            std::collections::btree_map::Entry::Vacant(vacant) => {
                vacant.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut occupied) => {
                if prefers(&candidate, occupied.get()) {
                    occupied.insert(candidate);
                }
            }
        }
    }
    let mut sets = Vec::new();
    for ((rank, _), volumes) in groups {
        let Some(first) = volumes.get(&0) else {
            continue;
        };
        if !first.is_first {
            continue;
        }
        let contiguous = volumes
            .keys()
            .enumerate()
            .all(|(position, index)| u32::try_from(position).is_ok_and(|p| p == *index));
        if !contiguous {
            continue;
        }
        let base = first
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(parse_archive_volume)
            .map(|volume| volume.base)
            .unwrap_or_default();
        sets.push(ArchiveSet {
            kind: kind_from_rank(rank),
            base,
            volumes: volumes
                .into_values()
                .map(|candidate| candidate.path)
                .collect(),
        });
    }
    sets
}

/// One name laying claim to a volume slot.
struct Candidate {
    path: PathBuf,
    is_first: bool,
    /// Whether the name states its own number (`part01`) rather than being a bare `.rar`.
    numbered: bool,
}

/// Whether `candidate` should replace `current` in the same volume slot.
///
/// An explicitly numbered name beats an unnumbered one (`part01` over a bare `.rar`); otherwise
/// the smaller file name wins, which only has to be stable, not meaningful.
fn prefers(candidate: &Candidate, current: &Candidate) -> bool {
    match (candidate.numbered, current.numbered) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.path.file_name() < current.path.file_name(),
    }
}

const fn kind_rank(kind: ArchiveKind) -> u8 {
    match kind {
        ArchiveKind::Zip => 0,
        ArchiveKind::SevenZip => 1,
        ArchiveKind::Rar => 2,
    }
}

const fn kind_from_rank(rank: u8) -> ArchiveKind {
    match rank {
        0 => ArchiveKind::Zip,
        1 => ArchiveKind::SevenZip,
        _ => ArchiveKind::Rar,
    }
}

/// Presents ordered volume files as one contiguous, seekable stream.
pub struct MultiVolumeReader {
    volumes: Vec<(PathBuf, u64)>,
    offsets: Vec<u64>,
    total: u64,
    position: u64,
    open: Option<(usize, File)>,
}

impl MultiVolumeReader {
    /// Opens every volume once to determine its size.
    pub fn open(volumes: &[PathBuf]) -> Result<Self> {
        let mut sized = Vec::with_capacity(volumes.len());
        let mut offsets = Vec::with_capacity(volumes.len());
        let mut total = 0_u64;
        for path in volumes {
            let length = std::fs::metadata(path)
                .with_context(|| format!("stat archive volume {}", path.display()))?
                .len();
            offsets.push(total);
            total = total
                .checked_add(length)
                .context("archive volume size overflow")?;
            sized.push((path.clone(), length));
        }
        Ok(Self {
            volumes: sized,
            offsets,
            total,
            position: 0,
            open: None,
        })
    }

    fn locate(&self, position: u64) -> Option<usize> {
        if position >= self.total {
            return None;
        }
        let index = self.offsets.partition_point(|offset| *offset <= position);
        index.checked_sub(1)
    }
}

impl Read for MultiVolumeReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(index) = self.locate(self.position) else {
            return Ok(0);
        };
        if self.open.as_ref().is_none_or(|(open, _)| *open != index) {
            self.open = Some((index, File::open(&self.volumes[index].0)?));
        }
        let (_, length) = &self.volumes[index];
        let local = self.position - self.offsets[index];
        let remaining = length - local;
        let (_, file) = self.open.as_mut().expect("volume opened above");
        file.seek(SeekFrom::Start(local))?;
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = file.read(&mut buffer[..wanted])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for MultiVolumeReader {
    fn seek(&mut self, target: SeekFrom) -> std::io::Result<u64> {
        let next = match target {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.total) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start of archive set",
            ));
        }
        self.position = u64::try_from(next).unwrap_or(u64::MAX);
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::group_archive_sets;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from(format!("/d/{name}")))
            .collect()
    }

    #[test]
    fn groups_complete_sets_and_drops_incomplete_ones() {
        let sets = group_archive_sets(&paths(&[
            "a.part2.rar",
            "a.part1.rar",
            "a.part3.rar",
            "b.rar",
            "b.r00",
            "c.7z.002", // no .001 → dropped
            "d.zip",
            "movie.mkv",
        ]));
        let names: Vec<(String, usize)> = sets
            .iter()
            .map(|set| (set.base.clone(), set.volumes.len()))
            .collect();
        assert_eq!(
            names,
            [
                ("d".to_owned(), 1),
                ("a".to_owned(), 3),
                ("b".to_owned(), 2)
            ]
        );
        assert!(sets[1].first().ends_with("a.part1.rar"));
    }

    #[test]
    fn a_numbered_entry_volume_wins_over_a_bare_rar_in_either_input_order() {
        // RD-107-11: `Release.rar` and `Release.part01.rar` are both index 0. Whichever the
        // directory listing yielded first used to decide where extraction started.
        for names in [
            ["Release.rar", "Release.part01.rar", "Release.part02.rar"],
            ["Release.part01.rar", "Release.rar", "Release.part02.rar"],
        ] {
            let sets = group_archive_sets(&paths(&names));
            assert_eq!(sets.len(), 1, "{names:?}");
            assert!(
                sets[0].first().ends_with("Release.part01.rar"),
                "{:?} for {names:?}",
                sets[0].first()
            );
            assert_eq!(sets[0].volumes.len(), 2, "{names:?}");
        }
    }

    #[test]
    fn a_tie_between_two_unnumbered_names_is_broken_by_the_file_name() {
        let forward = group_archive_sets(&paths(&["Release.rar", "release.RAR"]));
        let backward = group_archive_sets(&paths(&["release.RAR", "Release.rar"]));
        assert_eq!(forward[0].first(), backward[0].first());
    }

    #[test]
    fn gaps_disqualify_a_set() {
        let sets = group_archive_sets(&paths(&["x.part1.rar", "x.part3.rar"]));
        assert!(sets.is_empty());
    }
}
