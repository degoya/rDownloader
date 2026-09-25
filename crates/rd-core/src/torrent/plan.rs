//! The file plan of a torrent: which files are downloaded, in which priority tier, and
//! which exclusion patterns apply.
//!
//! Selection, patterns and priority can contradict each other, so the resolution in
//! [`resolve_plan`] follows five documented rules:
//!
//! 1. An explicit per-file decision by the user beats every pattern.
//! 2. Patterns only apply to files the user has never touched explicitly.
//! 3. Priority [`TorrentFilePriority::Skip`] is equivalent to deselecting the file.
//! 4. A file excluded by a pattern keeps its stored priority, so re-including it restores
//!    the previous state unchanged.
//! 5. A folder priority is inherited onto its files at the time it is set; only per-file
//!    priorities are persisted.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::metadata::TorrentMetadataInfo;

/// Maximum number of exclusion patterns per torrent.
pub const MAX_EXCLUSION_PATTERNS: usize = 64;

/// Maximum accepted length of one exclusion pattern.
pub const MAX_EXCLUSION_PATTERN_LENGTH: usize = 256;

/// Download priority of a single file. Ordering is meaningful: `High` is the largest.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TorrentFilePriority {
    /// Never download this file.
    Skip,
    Low,
    #[default]
    Normal,
    High,
}

impl TorrentFilePriority {
    /// Tiers in the order the engine opens them.
    #[must_use]
    pub const fn tiers() -> [Self; 3] {
        [Self::High, Self::Normal, Self::Low]
    }
}

/// Streaming-oriented piece ordering. Only [`TorrentSequentialMode::Off`] is supported by
/// the embedded engine; the other variants exist so a future engine only has to flip a
/// capability flag.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TorrentSequentialMode {
    #[default]
    Off,
    /// Download pieces front to back.
    Sequential,
    /// Prioritise the first and last piece of every selected file.
    FirstLast,
}

/// Persisted plan of one torrent. Empty defaults mean "download everything at normal
/// priority", which is what an untouched torrent gets.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct TorrentFilePlan {
    /// Files the user explicitly included (`true`) or excluded (`false`).
    pub explicit: BTreeMap<u32, bool>,
    /// Per-file priority; absent means [`TorrentFilePriority::Normal`].
    pub priorities: BTreeMap<u32, TorrentFilePriority>,
    pub exclusion_patterns: Vec<String>,
    pub sequential: TorrentSequentialMode,
}

impl TorrentFilePlan {
    /// Whether the plan carries any user decision at all.
    #[must_use]
    pub fn is_untouched(&self) -> bool {
        self.explicit.is_empty()
            && self.priorities.is_empty()
            && self.exclusion_patterns.is_empty()
            && matches!(self.sequential, TorrentSequentialMode::Off)
    }
}

/// The resolved state of one file after applying selection, patterns and priority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentFileDecision {
    pub index: u32,
    pub path: Vec<String>,
    pub length: crate::ByteCount,
    pub included: bool,
    pub priority: TorrentFilePriority,
    /// Set when rule 2 excluded the file; carries the pattern that matched.
    pub excluded_by_pattern: Option<String>,
    /// Whether the user decided this file explicitly (rule 1).
    pub explicit: bool,
}

/// The full resolution result for one torrent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ResolvedTorrentPlan {
    pub files: Vec<TorrentFileDecision>,
    pub selected_bytes: crate::ByteCount,
    pub total_bytes: crate::ByteCount,
    pub sequential: TorrentSequentialMode,
}

impl ResolvedTorrentPlan {
    /// Indices the engine should download.
    #[must_use]
    pub fn included_indices(&self) -> BTreeSet<u32> {
        self.files
            .iter()
            .filter(|file| file.included)
            .map(|file| file.index)
            .collect()
    }

    /// Included indices of exactly one priority tier.
    #[must_use]
    pub fn indices_in_tier(&self, tier: TorrentFilePriority) -> BTreeSet<u32> {
        self.files
            .iter()
            .filter(|file| file.included && file.priority == tier)
            .map(|file| file.index)
            .collect()
    }
}

/// Applies the five conflict rules to one torrent's metadata and plan.
#[must_use]
pub fn resolve_plan(metadata: &TorrentMetadataInfo, plan: &TorrentFilePlan) -> ResolvedTorrentPlan {
    let files: Vec<TorrentFileDecision> = metadata
        .files
        .iter()
        .map(|file| {
            let priority = plan
                .priorities
                .get(&file.index)
                .copied()
                .unwrap_or_default();
            let path = file.display_path();
            // Rule 4: the pattern verdict is computed regardless of the outcome so it stays
            // visible in the preview even when rule 1 overrides it.
            let matched_pattern = plan
                .exclusion_patterns
                .iter()
                .find(|pattern| glob_match(pattern, &path))
                .cloned();
            let explicit = plan.explicit.get(&file.index).copied();
            // Rules 1 and 2.
            let mut included = match explicit {
                Some(decision) => decision,
                None => matched_pattern.is_none(),
            };
            // Rule 3.
            if priority == TorrentFilePriority::Skip {
                included = false;
            }
            TorrentFileDecision {
                index: file.index,
                path: file.path.clone(),
                length: file.length,
                included,
                priority,
                excluded_by_pattern: matched_pattern.filter(|_| explicit.is_none()),
                explicit: explicit.is_some(),
            }
        })
        .collect();
    let selected: u64 = files
        .iter()
        .filter(|file| file.included)
        .map(|file| file.length.get())
        .sum();
    ResolvedTorrentPlan {
        selected_bytes: crate::ByteCount::new(selected).unwrap_or_default(),
        total_bytes: metadata.total_bytes,
        sequential: plan.sequential,
        files,
    }
}

/// Matches one exclusion pattern against a relative torrent path.
///
/// A pattern without `/` matches the file name; a pattern with `/` matches the whole
/// relative path. `*` matches within one path segment, `**` matches across segments, `?`
/// matches one character. Matching is case-insensitive so `*.nfo` also catches `*.NFO`.
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let path = path.to_lowercase();
    let subject = if pattern.contains('/') {
        path.as_str()
    } else {
        path.rsplit('/').next().unwrap_or(path.as_str())
    };
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let subject_segments: Vec<&str> = subject.split('/').collect();
    match_segments(&pattern_segments, &subject_segments)
}

fn match_segments(pattern: &[&str], subject: &[&str]) -> bool {
    match pattern.split_first() {
        None => subject.is_empty(),
        Some((&"**", rest)) => {
            // `**` consumes any number of segments, including none.
            (0..=subject.len()).any(|skip| match_segments(rest, &subject[skip..]))
        }
        Some((head, rest)) => match subject.split_first() {
            Some((first, remaining)) if match_segment(head, first) => {
                match_segments(rest, remaining)
            }
            _ => false,
        },
    }
}

/// Wildcard match inside one path segment.
fn match_segment(pattern: &str, subject: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let subject: Vec<char> = subject.chars().collect();
    let (mut pattern_index, mut subject_index) = (0_usize, 0_usize);
    let (mut star_index, mut star_subject) = (None, 0_usize);
    while subject_index < subject.len() {
        match pattern.get(pattern_index) {
            Some('*') => {
                star_index = Some(pattern_index);
                star_subject = subject_index;
                pattern_index += 1;
            }
            Some('?') => {
                pattern_index += 1;
                subject_index += 1;
            }
            Some(character) if *character == subject[subject_index] => {
                pattern_index += 1;
                subject_index += 1;
            }
            _ => match star_index {
                // Backtrack: let the last `*` swallow one more character.
                Some(index) => {
                    pattern_index = index + 1;
                    star_subject += 1;
                    subject_index = star_subject;
                }
                None => return false,
            },
        }
    }
    pattern[pattern_index..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        TorrentFilePlan, TorrentFilePriority, TorrentSequentialMode, glob_match, resolve_plan,
    };
    use crate::{
        ByteCount,
        torrent::metadata::{TorrentFileEntry, TorrentMetadataInfo},
    };

    fn metadata() -> TorrentMetadataInfo {
        let files = vec![
            ("movie.mkv", 1_000_u64),
            ("sample/sample.mkv", 10),
            ("extras/behind.nfo", 5),
            ("extras/poster.jpg", 20),
        ];
        TorrentMetadataInfo {
            info_hash: "hash".to_owned(),
            name: "release".to_owned(),
            total_bytes: ByteCount::new(1_035).expect("valid"),
            piece_length: 16_384,
            piece_count: 64,
            private: false,
            files: files
                .into_iter()
                .enumerate()
                .map(|(index, (path, length))| TorrentFileEntry {
                    index: u32::try_from(index).expect("small"),
                    path: path.split('/').map(str::to_owned).collect(),
                    length: ByteCount::new(length).expect("valid"),
                })
                .collect(),
            trackers: Vec::new(),
            web_seeds: Vec::new(),
        }
    }

    #[test]
    fn an_untouched_plan_selects_everything() {
        let resolved = resolve_plan(&metadata(), &TorrentFilePlan::default());
        assert_eq!(resolved.included_indices().len(), 4);
        assert_eq!(resolved.selected_bytes.get(), 1_035);
    }

    #[test]
    fn patterns_exclude_untouched_files_only() {
        let plan = TorrentFilePlan {
            exclusion_patterns: vec!["*.nfo".to_owned()],
            ..TorrentFilePlan::default()
        };
        let resolved = resolve_plan(&metadata(), &plan);
        assert!(!resolved.included_indices().contains(&2));
        assert_eq!(
            resolved.files[2].excluded_by_pattern.as_deref(),
            Some("*.nfo")
        );
    }

    #[test]
    fn an_explicit_decision_beats_a_matching_pattern() {
        let plan = TorrentFilePlan {
            explicit: BTreeMap::from([(2, true)]),
            exclusion_patterns: vec!["*.nfo".to_owned()],
            ..TorrentFilePlan::default()
        };
        let resolved = resolve_plan(&metadata(), &plan);
        assert!(resolved.included_indices().contains(&2));
        // Rule 1 wins, and the pattern is not reported as the reason.
        assert!(resolved.files[2].excluded_by_pattern.is_none());
        assert!(resolved.files[2].explicit);
    }

    #[test]
    fn skip_priority_deselects_even_an_explicit_include() {
        let plan = TorrentFilePlan {
            explicit: BTreeMap::from([(1, true)]),
            priorities: BTreeMap::from([(1, TorrentFilePriority::Skip)]),
            ..TorrentFilePlan::default()
        };
        let resolved = resolve_plan(&metadata(), &plan);
        assert!(!resolved.included_indices().contains(&1));
    }

    #[test]
    fn a_pattern_excluded_file_keeps_its_priority() {
        let plan = TorrentFilePlan {
            priorities: BTreeMap::from([(2, TorrentFilePriority::High)]),
            exclusion_patterns: vec!["*.nfo".to_owned()],
            ..TorrentFilePlan::default()
        };
        let resolved = resolve_plan(&metadata(), &plan);
        assert!(!resolved.files[2].included);
        assert_eq!(resolved.files[2].priority, TorrentFilePriority::High);
    }

    #[test]
    fn tiers_group_the_included_files() {
        let plan = TorrentFilePlan {
            priorities: BTreeMap::from([(0, TorrentFilePriority::High)]),
            ..TorrentFilePlan::default()
        };
        let resolved = resolve_plan(&metadata(), &plan);
        assert_eq!(
            resolved.indices_in_tier(TorrentFilePriority::High),
            [0].into_iter().collect()
        );
        assert_eq!(
            resolved.indices_in_tier(TorrentFilePriority::Normal).len(),
            3
        );
        assert_eq!(resolved.sequential, TorrentSequentialMode::Off);
    }

    #[test]
    fn globs_match_names_paths_and_subtrees() {
        assert!(glob_match("*.nfo", "extras/behind.nfo"));
        assert!(glob_match("*.NFO", "extras/behind.nfo"));
        assert!(glob_match("extras/*", "extras/poster.jpg"));
        assert!(!glob_match("extras/*", "sample/sample.mkv"));
        assert!(glob_match("**/sample.mkv", "sample/sample.mkv"));
        assert!(glob_match("**", "any/deep/path.txt"));
        assert!(glob_match("mo?ie.mkv", "movie.mkv"));
        assert!(!glob_match("extras/*.jpg", "extras/behind.nfo"));
        // A slashless pattern matches the file name, not the folder.
        assert!(!glob_match("extras", "extras/poster.jpg"));
    }
}
