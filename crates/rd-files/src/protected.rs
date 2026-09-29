//! Directories a storage root may not reach (security review 2026-09-28, finding 4).
//!
//! A storage root is where downloads, package folders and unpacked archives land, under names a
//! download chooses. A root at the data directory makes a package named `scripts` the scripts
//! directory, and a file downloaded into it a script a category may run; a root holding the
//! vendor folder turns a download into the `unrar` the service executes. Configuring a root
//! costs `api:config`, naming a program to run costs `api:admin`, and this is what keeps the
//! first from buying the second.

use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

/// A directory the service executes, loads or keeps its state from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedDirectory {
    /// Which one it is, as the API names it: `data`, `scripts`, `vendor`, `tools`, `plugins`,
    /// `program`.
    pub kind: &'static str,
    pub path: PathBuf,
    /// Whether a root *inside* it is refused as well. True for every directory except the
    /// program folder, whose tools are looked up at its top level only — and next to which a
    /// portable installation keeps its `downloads` folder.
    pub subtree: bool,
}

impl ProtectedDirectory {
    #[must_use]
    pub fn new(kind: &'static str, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            path: path.into(),
            subtree: true,
        }
    }

    /// Refused when it equals or contains the directory, allowed inside it.
    #[must_use]
    pub fn top_level_only(mut self) -> Self {
        self.subtree = false;
        self
    }
}

/// The protected directory a storage root at `candidate` would reach, if any: one it equals,
/// one it contains, or one it lies inside.
///
/// Both sides are compared resolved: symlinks followed as far as the path exists, `..` taken
/// away, and on Windows without regard to case. A path that does not exist yet is judged by
/// its nearest existing ancestor, so the check can run before the directory is created.
#[must_use]
pub fn protected_collision<'a>(
    candidate: &Path,
    protected: &'a [ProtectedDirectory],
) -> Option<&'a ProtectedDirectory> {
    let candidate = resolved(candidate);
    protected.iter().find(|directory| {
        let reserved = resolved(&directory.path);
        reserved.starts_with(&candidate) || (directory.subtree && candidate.starts_with(&reserved))
    })
}

/// `path` made absolute, `.` and `..` removed, and the longest existing prefix canonicalised.
fn resolved(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut existing = lexical.as_path();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if let Ok(mut canonical) = dunce::canonicalize(existing) {
            for part in missing.iter().rev() {
                canonical.push(part);
            }
            return comparable(canonical);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                existing = parent;
            }
            _ => return comparable(lexical),
        }
    }
}

#[cfg(windows)]
fn comparable(path: PathBuf) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
fn comparable(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Vec<ProtectedDirectory>) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let base = temporary.path();
        std::fs::create_dir_all(base.join("data/plugins")).expect("plugins");
        let protected = vec![
            ProtectedDirectory::new("data", base.join("data")),
            // Deliberately missing on disk: the scripts directory is created on first use.
            ProtectedDirectory::new("scripts", base.join("data/scripts")),
            ProtectedDirectory::new("vendor", base.join("app/vendor")),
            ProtectedDirectory::new("plugins", base.join("data/plugins")),
            ProtectedDirectory::new("program", base.join("app")).top_level_only(),
        ];
        (temporary, protected)
    }

    fn kind<'a>(candidate: &Path, protected: &'a [ProtectedDirectory]) -> Option<&'a str> {
        protected_collision(candidate, protected).map(|directory| directory.kind)
    }

    #[test]
    fn a_root_equal_to_a_protected_directory_is_refused() {
        let (temporary, protected) = layout();
        let base = temporary.path();
        assert_eq!(kind(&base.join("data"), &protected), Some("data"));
        assert_eq!(kind(&base.join("data/scripts"), &protected), Some("data"));
        assert_eq!(kind(&base.join("app/vendor"), &protected), Some("vendor"));
        assert_eq!(kind(&base.join("app"), &protected), Some("vendor"));
    }

    #[test]
    fn a_root_containing_a_protected_directory_is_refused() {
        let (temporary, protected) = layout();
        assert_eq!(kind(temporary.path(), &protected), Some("data"));
        // The root of the file system the layout lies on: on Windows `/` would be the root of
        // the current drive, which need not be the drive of the temporary directory.
        let root = temporary.path().ancestors().last().expect("a root");
        assert_eq!(kind(root, &protected), Some("data"));
    }

    #[test]
    fn a_root_inside_a_protected_directory_is_refused() {
        let (temporary, protected) = layout();
        let base = temporary.path();
        assert_eq!(kind(&base.join("data/downloads"), &protected), Some("data"));
        assert_eq!(kind(&base.join("app/vendor/x"), &protected), Some("vendor"));
    }

    /// The program folder is protected at its top level only: `downloads` next to the binary is
    /// the portable layout, and nothing is looked up below the folder itself.
    #[test]
    fn a_root_inside_the_program_folder_is_allowed() {
        let (temporary, protected) = layout();
        let base = temporary.path();
        assert_eq!(kind(&base.join("app/downloads"), &protected), None);
        assert_eq!(kind(&base.join("downloads"), &protected), None);
    }

    #[test]
    fn dot_dot_does_not_walk_around_the_check() {
        let (temporary, protected) = layout();
        let sneaky = temporary.path().join("downloads/../data/scripts");
        assert_eq!(kind(&sneaky, &protected), Some("data"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_protected_directory_is_refused() {
        let (temporary, protected) = layout();
        let link = temporary.path().join("innocent");
        std::os::unix::fs::symlink(temporary.path().join("data/plugins"), &link).expect("link");
        assert_eq!(kind(&link, &protected), Some("data"));
        assert_eq!(kind(&link.join("sub"), &protected), Some("data"));
    }
}
