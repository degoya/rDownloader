//! The executable path a package manager keeps valid across its updates.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

/// The path a package manager keeps valid across updates for `executable`, or `executable`
/// itself when it has none.
///
/// Scoop installs into `<root>\apps\<app>\<version>\` and points the junction
/// `<root>\apps\<app>\current\` at the newest one; Homebrew installs into
/// `<prefix>/Cellar/<formula>/<version>/` and links `<prefix>/opt/<formula>/` to it. A
/// registration that stored the versioned path — the autostart entry, the URL-scheme handler,
/// the file association — keeps starting the old version after an update and breaks once the
/// cleanup deletes its folder (`scoop cleanup`, `brew cleanup`, which `brew upgrade` runs). The
/// alias is only taken when the same file exists under it; `apps` is matched without regard to
/// case, as Windows paths are.
pub fn stable_executable_path(executable: &Path) -> PathBuf {
    let parts: Vec<Component<'_>> = executable.components().collect();
    // Innermost first: the folder nearest the executable is the one its package manager owns.
    for index in (0..parts.len()).rev() {
        let Component::Normal(name) = parts[index] else {
            continue;
        };
        // `<package>`, `<version>` and at least the executable below it.
        if parts.len() < index + 4 {
            continue;
        }
        let (head, package, rest) = (&parts[..index], parts[index + 1], &parts[index + 3..]);
        let alias = if name.eq_ignore_ascii_case("apps") {
            vec![
                parts[index],
                package,
                Component::Normal(OsStr::new("current")),
            ]
        } else if name == "Cellar" {
            vec![Component::Normal(OsStr::new("opt")), package]
        } else {
            continue;
        };
        let candidate: PathBuf = head.iter().chain(&alias).chain(rest).collect();
        if candidate.exists() {
            return candidate;
        }
    }
    executable.to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::stable_executable_path;

    fn touch(root: &Path, relative: &[&str]) -> PathBuf {
        let path = relative
            .iter()
            .fold(root.to_owned(), |path, part| path.join(part));
        let parent = path.parent().expect("a parent directory");
        std::fs::create_dir_all(parent).expect("create the layout");
        std::fs::write(&path, b"").expect("write the executable");
        path
    }

    #[test]
    fn a_scoop_version_folder_resolves_to_the_current_junction() {
        let root = tempfile::tempdir().expect("temporary directory");
        let versioned = touch(
            root.path(),
            &[
                "Scoop",
                "Apps",
                "rdownloader",
                "1.7.0",
                "rdownloader-capture.exe",
            ],
        );
        let current = touch(
            root.path(),
            &[
                "Scoop",
                "Apps",
                "rdownloader",
                "current",
                "rdownloader-capture.exe",
            ],
        );
        assert_eq!(stable_executable_path(&versioned), current);
        assert_eq!(stable_executable_path(&current), current);
    }

    #[test]
    fn a_homebrew_cellar_path_resolves_to_the_opt_link() {
        let root = tempfile::tempdir().expect("temporary directory");
        let versioned = touch(
            root.path(),
            &[
                "homebrew",
                "Cellar",
                "rdownloader",
                "1.7.0",
                "libexec",
                "rdownloader",
            ],
        );
        let opt = touch(
            root.path(),
            &["homebrew", "opt", "rdownloader", "libexec", "rdownloader"],
        );
        assert_eq!(stable_executable_path(&versioned), opt);
    }

    #[test]
    fn without_a_stable_alias_the_path_is_unchanged() {
        let root = tempfile::tempdir().expect("temporary directory");
        for relative in [
            &["scoop", "apps", "rdownloader", "1.7.0", "rdownloader.exe"][..],
            &[
                "homebrew",
                "Cellar",
                "rdownloader",
                "1.7.0",
                "libexec",
                "rdownloader",
            ][..],
            &["Tools", "rdownloader", "rdownloader-capture.exe"][..],
        ] {
            let executable = touch(root.path(), relative);
            assert_eq!(stable_executable_path(&executable), executable);
        }
        // The alias folder exists, but not the file under it: nothing is registered that would
        // not start.
        let versioned = touch(
            root.path(),
            &[
                "other",
                "apps",
                "rdownloader",
                "1.7.0",
                "rdownloader-capture.exe",
            ],
        );
        touch(
            root.path(),
            &["other", "apps", "rdownloader", "current", "README.md"],
        );
        assert_eq!(stable_executable_path(&versioned), versioned);
    }
}
