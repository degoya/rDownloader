//! Where a build an installer put in place keeps its data (RD-180-05).
//!
//! The portable package keeps everything beside the executable: its launcher starts the service
//! there, and the relative defaults (`data/`, `downloads`) resolve in that folder. An installer
//! puts the executable where the user may not write (`/usr/lib/rdownloader`) or where an upgrade
//! replaces the folder (the MSI's program folder), so it writes a marker file beside the
//! executable, and the data moves to the user's own data folder instead.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The marker an installer writes beside the executable: one word naming the installer (`msi`,
/// `deb`, `rpm`). The update check reads the same file to tell the kinds apart.
pub const INSTALL_KIND_FILE: &str = "install-kind";

/// The installer that put `executable` in place, or `None` for a portable copy.
pub fn install_kind(executable: &Path) -> Result<Option<String>> {
    let Some(directory) = executable.parent() else {
        return Ok(None);
    };
    let marker = directory.join(INSTALL_KIND_FILE);
    let text = match std::fs::read_to_string(&marker) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", marker.display())),
    };
    let kind = text.trim();
    // A marker is written by an installer, never by hand; anything else is a broken package,
    // and guessing a data folder for it could open the wrong database.
    if kind.is_empty()
        || !kind.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        bail!("{} does not name an installer", marker.display());
    }
    Ok(Some(kind.to_owned()))
}

/// The data folder of an installed build, or `None` for a portable one.
///
/// `%LOCALAPPDATA%\rDownloader` on Windows, `$XDG_DATA_HOME/rdownloader` (by default
/// `~/.local/share/rdownloader`) on Linux. No installer ever removes it.
pub fn installed_home(executable: &Path) -> Result<Option<PathBuf>> {
    if install_kind(executable)?.is_none() {
        return Ok(None);
    }
    let base = directories::BaseDirs::new().context("locate the user's data folder")?;
    Ok(Some(data_home(&base)))
}

#[cfg(windows)]
fn data_home(base: &directories::BaseDirs) -> PathBuf {
    base.data_local_dir().join("rDownloader")
}

#[cfg(not(windows))]
fn data_home(base: &directories::BaseDirs) -> PathBuf {
    base.data_dir().join("rdownloader")
}

#[cfg(test)]
mod tests {
    use super::{INSTALL_KIND_FILE, install_kind, installed_home};

    #[test]
    fn a_portable_copy_has_no_kind_and_no_data_folder_of_its_own() {
        let root = tempfile::tempdir().expect("temporary directory");
        let executable = root.path().join("rdownloader");
        assert_eq!(install_kind(&executable).expect("no marker"), None);
        assert_eq!(installed_home(&executable).expect("no marker"), None);
    }

    #[test]
    fn the_marker_names_the_installer_and_moves_the_data_to_the_user() {
        let root = tempfile::tempdir().expect("temporary directory");
        let executable = root.path().join("rdownloader");
        for kind in ["msi", "deb", "rpm"] {
            std::fs::write(root.path().join(INSTALL_KIND_FILE), format!("{kind}\n"))
                .expect("write the marker");
            assert_eq!(
                install_kind(&executable).expect("a marker"),
                Some(kind.to_owned())
            );
            let home = installed_home(&executable)
                .expect("a marker")
                .expect("a data folder");
            // Never the folder the installer owns: an upgrade replaces it.
            assert!(!home.starts_with(root.path()), "{}", home.display());
            let name = home.file_name().and_then(|name| name.to_str());
            assert!(
                matches!(name, Some("rdownloader" | "rDownloader")),
                "{}",
                home.display()
            );
        }
    }

    #[test]
    fn a_marker_that_names_no_installer_is_refused() {
        let root = tempfile::tempdir().expect("temporary directory");
        let executable = root.path().join("rdownloader");
        for text in ["", "\n", "MSI", "../data", "msi deb"] {
            std::fs::write(root.path().join(INSTALL_KIND_FILE), text).expect("write the marker");
            assert!(install_kind(&executable).is_err(), "accepted {text:?}");
        }
    }
}
