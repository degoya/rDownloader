//! Cleanup after unpacking: configured extensions and sample files inside the package folder.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessState};

use crate::{
    Inner,
    steps::{checkpoint, truncate},
};

/// Folder levels the cleanup walks, counted from where unpacked content starts.
const MAX_DEPTH: usize = 4;

/// How many folder levels below the package folder the cleanup walks.
///
/// [`MAX_DEPTH`], and one more when every archive was unpacked into a folder of its own
/// (RD-170-16): that folder is a level the content did not have, and without it the content of
/// an archive was cleaned one level less deep with the option on than off (RD-190-06).
pub(crate) fn depth(own_folders: bool) -> usize {
    MAX_DEPTH + usize::from(own_folders)
}

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

/// The unwanted files below `directory`, at most `max_depth` folder levels down (the package
/// folder is level 0; no symlinks, no escaping).
pub(crate) fn collect_targets(
    directory: &Path,
    rules: &CleanupRules,
    max_depth: usize,
) -> Vec<PathBuf> {
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
                if depth + 1 < max_depth {
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

/// Removes the targets and records the step; returns the removed files relative to `directory`,
/// `/` between folders.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    directory: &Path,
    rules: &CleanupRules,
    max_depth: usize,
) -> Result<Vec<String>> {
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
    let targets = collect_targets(directory, rules, max_depth);
    let root = dunce::canonicalize(directory).ok();
    let mut gone = Vec::new();
    let mut errors = Vec::new();
    for path in &targets {
        match tokio::fs::remove_file(path).await {
            Ok(()) => gone.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    let removed = gone.len();
    let folders = match &root {
        Some(root) => remove_emptied_folders(root, &targets).await,
        None => 0,
    };
    let (state, message) = if errors.is_empty() {
        (
            PostprocessState::Completed,
            format!("removed={removed} folders={folders}"),
        )
    } else {
        (
            PostprocessState::Failed,
            truncate(format!(
                "removed={removed} folders={folders} errors={}",
                errors.join("; ")
            )),
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
    .await?;
    // Named the way a plugin step is offered the package, for the step after this one
    // (RD-190-06). `collect_targets` walks from the canonical root, so every path is below it.
    Ok(root.map_or_else(Vec::new, |root| {
        gone.into_iter()
            .filter_map(|path| path.strip_prefix(&root).ok())
            .map(crate::plugin_step::relative_name)
            .collect()
    }))
}

/// Removes the folders the cleanup left empty — a `Sample` folder whose only file was the
/// sample — walking up from each removed file's folder, never the package folder itself.
/// `remove_dir` refuses a folder that still holds anything, so a folder with other content, or
/// one the cleanup did not empty, stays. Returns how many folders went.
async fn remove_emptied_folders(root: &Path, removed: &[PathBuf]) -> usize {
    let mut folders: Vec<&Path> = removed.iter().filter_map(|path| path.parent()).collect();
    folders.sort();
    folders.dedup();
    // Deepest first, so a folder is tried after everything below it.
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    let mut count = 0;
    for folder in folders {
        let mut current = Some(folder);
        while let Some(dir) = current.filter(|dir| *dir != root && dir.starts_with(root)) {
            if tokio::fs::remove_dir(dir).await.is_err() {
                break;
            }
            count += 1;
            current = dir.parent();
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{CleanupRules, MAX_DEPTH, collect_targets, depth, remove_emptied_folders};

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
        let targets = collect_targets(&root, &rules(), MAX_DEPTH);
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

    #[tokio::test]
    async fn a_folder_the_cleanup_emptied_goes_with_its_sample() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("pkg");
        std::fs::create_dir_all(root.join("Sample")).expect("sample folder");
        std::fs::create_dir_all(root.join("Extras")).expect("extras folder");
        std::fs::create_dir_all(root.join("Empty")).expect("empty folder");
        std::fs::write(root.join("movie.mkv"), vec![0; 2048]).expect("mkv");
        std::fs::write(root.join("Sample/movie-sample.mkv"), b"tiny").expect("sample");
        std::fs::write(root.join("Extras/extra.nfo"), b"x").expect("nfo");
        std::fs::write(root.join("Extras/interview.mkv"), vec![0; 2048]).expect("extra");
        let root = dunce::canonicalize(&root).expect("root");
        let targets = collect_targets(&root, &rules(), MAX_DEPTH);
        for path in &targets {
            std::fs::remove_file(path).expect("remove target");
        }

        assert_eq!(remove_emptied_folders(&root, &targets).await, 1);
        assert!(
            !root.join("Sample").exists(),
            "the emptied sample folder goes"
        );
        assert!(
            root.join("Extras/interview.mkv").exists(),
            "a folder with content stays"
        );
        assert!(
            root.join("Empty").is_dir(),
            "a folder the cleanup did not empty stays"
        );
        assert!(root.join("movie.mkv").exists());
        assert!(root.is_dir(), "the package folder itself stays");
    }

    /// RD-190-06: the same archive content, unpacked into the package folder or into a folder
    /// of its own, is cleaned to the same depth below the content.
    #[test]
    fn a_folder_per_archive_is_cleaned_as_deep_as_the_package_folder() {
        let deepest = ["a", "b", "c"];
        let too_deep = ["a", "b", "c", "d"];
        for (own_folders, prefix) in [(false, None), (true, Some("Film"))] {
            let temp = tempfile::tempdir().expect("tempdir");
            let root = temp.path().join("pkg");
            let content = prefix.map_or_else(|| root.clone(), |folder| root.join(folder));
            let reached = deepest
                .iter()
                .fold(content.clone(), |path, part| path.join(part));
            let beyond = too_deep
                .iter()
                .fold(content.clone(), |path, part| path.join(part));
            std::fs::create_dir_all(&beyond).expect("folders");
            std::fs::write(reached.join("release.nfo"), b"x").expect("nfo");
            std::fs::write(beyond.join("deeper.nfo"), b"x").expect("nfo");
            let root = dunce::canonicalize(&root).expect("root");

            let targets = collect_targets(&root, &rules(), depth(own_folders));

            let found: Vec<&str> = targets
                .iter()
                .filter_map(|path| path.file_name()?.to_str())
                .collect();
            assert_eq!(found, ["release.nfo"], "own folders: {own_folders}");
        }
    }
}
