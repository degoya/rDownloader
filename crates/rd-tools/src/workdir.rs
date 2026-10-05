//! The working directory every external tool starts in (RD-1101-16, audit S21).
//!
//! A tool started without one inherits the service's, and yt-dlp reads a `yt-dlp.conf` from
//! its working directory when no `-P` names another: whoever could put a file wherever the
//! service happened to be started handed every download options of their choosing - an
//! `--exec`, a proxy, an output path. The tools now start in a directory of their own instead:
//! created once per run of the service in the temporary directory, under a name nobody can
//! guess and, on Unix, `0700` from its creation. Nothing writes into it; every tool is given
//! absolute paths for what it writes.
//!
//! The configuration a tool reads from its own folders - the home and `XDG_CONFIG_HOME` files
//! [`rd_files::TOOL_VARIABLES`] keeps reachable on purpose - is untouched: that is the
//! operator's, set up for the account the service runs as, and a download that works from a
//! shell keeps working from the service.

use std::{
    fs::DirBuilder,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tokio::process::Command;

static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Starts `command` in the tools' own directory, unless its caller chose one.
///
/// A program named by a relative path with folders in it keeps the service's directory too:
/// where it is looked up from once the directory changes differs between platforms, and the
/// tool must be the one that was configured.
pub(crate) fn isolate(command: &mut Command) {
    let planned = command.as_std();
    if planned.get_current_dir().is_some()
        || relative_with_folders(Path::new(planned.get_program()))
    {
        return;
    }
    if let Some(directory) = DIRECTORY.get_or_init(create) {
        command.current_dir(directory);
    }
}

fn relative_with_folders(program: &Path) -> bool {
    program.is_relative() && program.components().count() > 1
}

/// The directory, or `None` when it cannot be made; the tools then start where they always
/// did, which is a finding's worth of risk and no reason to stop every download.
fn create() -> Option<PathBuf> {
    let directory =
        std::env::temp_dir().join(format!("rdownloader-tools-{}", uuid::Uuid::now_v7()));
    // `create`, not `create_all`: a directory someone else made first under this name is a
    // failure, never one to start the tools in.
    match private_builder().create(&directory) {
        Ok(()) => Some(directory),
        Err(error) => {
            tracing::warn!(
                %error,
                path = %directory.display(),
                "the external tools have no working directory of their own and start in the service's"
            );
            None
        }
    }
}

#[cfg(unix)]
fn private_builder() -> DirBuilder {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder
}

#[cfg(not(unix))]
fn private_builder() -> DirBuilder {
    DirBuilder::new()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::relative_with_folders;

    #[test]
    fn only_a_relative_path_with_folders_keeps_the_services_directory() {
        assert!(relative_with_folders(Path::new("vendor/yt-dlp")));
        assert!(relative_with_folders(Path::new("./yt-dlp")));
        assert!(!relative_with_folders(Path::new("yt-dlp")));
        assert!(!relative_with_folders(&std::env::temp_dir().join("yt-dlp")));
    }

    /// The finding: a tool ran in the service's working directory, where yt-dlp finds a
    /// `yt-dlp.conf` anyone who could write there had left.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_starts_in_an_empty_directory_of_its_own() {
        use std::os::unix::fs::PermissionsExt as _;

        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "pwd"]);
        let output = crate::run_to_output(&mut command, std::time::Duration::from_secs(10))
            .await
            .expect("in time")
            .expect("runs");
        let directory =
            std::path::PathBuf::from(String::from_utf8(output.stdout).expect("a path").trim_end());

        assert_ne!(
            resolved(&directory),
            resolved(&std::env::current_dir().expect("current directory"))
        );
        assert!(resolved(&directory).starts_with(resolved(&std::env::temp_dir())));
        assert_eq!(
            std::fs::read_dir(&directory).expect("readable").count(),
            0,
            "nothing in it for a tool to read"
        );
        let mode = std::fs::metadata(&directory)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// A caller that chose a directory keeps it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_the_caller_chose_is_kept() {
        let chosen = tempfile::tempdir().expect("temporary directory");
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "pwd"]).current_dir(chosen.path());
        let output = crate::run_to_output(&mut command, std::time::Duration::from_secs(10))
            .await
            .expect("in time")
            .expect("runs");
        let directory = String::from_utf8(output.stdout).expect("a path");
        assert_eq!(
            resolved(Path::new(directory.trim_end())),
            resolved(chosen.path())
        );
    }

    /// `pwd` prints the resolved path; a temporary directory behind a symlink (macOS) would
    /// otherwise compare unequal to itself.
    #[cfg(unix)]
    fn resolved(path: &Path) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}
