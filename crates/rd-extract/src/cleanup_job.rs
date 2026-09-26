//! Cleanup after unpacking: configured extensions and sample files inside the package folder.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessState};

use crate::{
    Inner,
    steps::{checkpoint, truncate},
};

const MAX_DEPTH: usize = 4;

/// What the cleanup removes.
#[derive(Clone, Debug)]
pub(crate) struct CleanupRules {
    pub extensions: Vec<String>,
    pub ignore_samples: bool,
    pub sample_max_bytes: u64,
}

impl CleanupRules {
    pub(crate) fn is_active(&self) -> bool {
        !self.extensions.is_empty() || self.ignore_samples
    }

    /// Sample detection: "sample" as a separate word in the stem, below the size threshold.
    pub(crate) fn is_sample(&self, path: &Path, size: u64) -> bool {
        if !self.ignore_samples || size >= self.sample_max_bytes {
            return false;
        }
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        stem.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "sample")
    }

    fn matches_extension(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .is_some_and(|ext| self.extensions.contains(&ext))
    }
}

/// Removes unwanted files below `directory` (bounded depth, no symlinks, no escaping).
/// Returns the removed paths.
pub(crate) fn collect_targets(directory: &Path, rules: &CleanupRules) -> Vec<PathBuf> {
    let Ok(root) = dunce::canonicalize(directory) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    let mut pending = vec![(root.clone(), 0_usize)];
    while let Some((current, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            if !metadata.is_file() || !path.starts_with(&root) {
                continue;
            }
            if rules.matches_extension(&path) || rules.is_sample(&path, metadata.len()) {
                targets.push(path);
            }
        }
    }
    targets.sort();
    targets
}

pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    directory: &Path,
    rules: &CleanupRules,
) -> Result<()> {
    crate::steps::stage(inner, owner, rd_core::PostprocessStage::Cleaning, None).await?;
    checkpoint(
        inner,
        owner,
        PostprocessKind::Cleanup,
        "cleanup",
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    let targets = collect_targets(directory, rules);
    let mut removed = 0_usize;
    let mut errors = Vec::new();
    for path in &targets {
        match tokio::fs::remove_file(path).await {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    let (state, message) = if errors.is_empty() {
        (PostprocessState::Completed, format!("removed={removed}"))
    } else {
        (
            PostprocessState::Failed,
            truncate(format!("removed={removed} errors={}", errors.join("; "))),
        )
    };
    checkpoint(
        inner,
        owner,
        PostprocessKind::Cleanup,
        "cleanup",
        state,
        None,
        Some(message),
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{CleanupRules, collect_targets};

    fn rules() -> CleanupRules {
        CleanupRules {
            extensions: vec!["nfo".to_owned(), "sfv".to_owned()],
            ignore_samples: true,
            sample_max_bytes: 1024,
        }
    }

    #[test]
    fn detects_samples_by_word_and_size() {
        let rules = rules();
        assert!(rules.is_sample(Path::new("movie-sample.mkv"), 10));
        assert!(rules.is_sample(Path::new("Sample_Movie.mkv"), 10));
        assert!(!rules.is_sample(Path::new("resample.mkv"), 10));
        assert!(!rules.is_sample(Path::new("movie-sample.mkv"), 4096));
    }

    #[test]
    fn collects_only_matching_regular_files_inside_the_folder() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("pkg");
        std::fs::create_dir_all(root.join("sub")).expect("dirs");
        std::fs::write(root.join("release.nfo"), b"x").expect("nfo");
        std::fs::write(root.join("sub/check.SFV"), b"x").expect("sfv");
        std::fs::write(root.join("movie.mkv"), vec![0; 2048]).expect("mkv");
        std::fs::write(root.join("movie.sample.mkv"), b"tiny").expect("sample");
        std::fs::write(temp.path().join("outside.nfo"), b"x").expect("outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("outside.nfo"), root.join("link.nfo"))
            .expect("symlink");
        let targets = collect_targets(&root, &rules());
        let names: Vec<PathBuf> = targets
            .iter()
            .map(|path| {
                path.strip_prefix(dunce::canonicalize(&root).expect("root"))
                    .expect("rel")
                    .to_path_buf()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                PathBuf::from("movie.sample.mkv"),
                PathBuf::from("release.nfo"),
                Path::new("sub").join("check.SFV"),
            ]
        );
    }
}
