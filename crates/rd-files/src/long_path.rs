//! Windows paths that are not subject to the 260-character limit (RD-108-30).
//!
//! The field: a release folder of 75 characters, an archive whose inner tree repeats that name
//! and a `.mkv` of the same name make a path of 247 characters — under the limit, and extracted
//! by hand without trouble. The extraction staging directory added its 27 characters on top,
//! and `unrar` answered exit 9, "cannot create", 275 characters in. The file was fine, the
//! password was right, the archive was ordinary.
//!
//! `\\?\` turns the limit off for one call. It is not a cosmetic prefix: the path has to be
//! fully qualified, use backslashes only and contain no `.` or `..`, because the kernel passes
//! it to the file system without normalising it. Anything that does not qualify is left as it
//! is, which is no worse than before.

use std::path::{Path, PathBuf};

/// The form of `path` that Windows will not truncate. Unchanged on every other platform.
#[must_use]
pub fn long_path(path: &Path) -> PathBuf {
    if cfg!(windows) {
        verbatim_windows_path(path).unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

/// The `\\?\` form of an absolute Windows path, or `None` when there is none.
///
/// Pure string work on purpose, so the rules are tested where the tests run rather than only
/// on the platform they are for.
#[must_use]
pub(crate) fn verbatim_windows_path(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        return Some(path.to_path_buf());
    }
    let normalised = text.replace('/', "\\");
    // A verbatim path is handed to the file system as it stands, so a `.` or `..` in it would
    // become a directory of that name rather than a step in the tree.
    if normalised
        .split('\\')
        .any(|component| component == "." || component == "..")
    {
        return None;
    }
    let bytes = normalised.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return Some(PathBuf::from(format!(r"\\?\{normalised}")));
    }
    // `\\server\share\...` becomes `\\?\UNC\server\share\...`.
    if let Some(rest) = normalised.strip_prefix(r"\\")
        && rest.split('\\').filter(|part| !part.is_empty()).count() >= 2
    {
        return Some(PathBuf::from(format!(r"\\?\UNC\{rest}")));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::verbatim_windows_path;

    fn verbatim(text: &str) -> Option<PathBuf> {
        verbatim_windows_path(Path::new(text))
    }

    #[test]
    fn a_drive_path_gets_the_prefix_that_lifts_the_limit() {
        assert_eq!(
            verbatim(r"D:\downloads\tv\release\file.mkv"),
            Some(PathBuf::from(r"\\?\D:\downloads\tv\release\file.mkv"))
        );
    }

    #[test]
    fn forward_slashes_become_backslashes_because_a_verbatim_path_takes_no_others() {
        assert_eq!(
            verbatim("D:/downloads/tv"),
            Some(PathBuf::from(r"\\?\D:\downloads\tv"))
        );
    }

    #[test]
    fn a_share_keeps_its_server_and_share_under_unc() {
        assert_eq!(
            verbatim(r"\\nas\media\tv\release"),
            Some(PathBuf::from(r"\\?\UNC\nas\media\tv\release"))
        );
    }

    #[test]
    fn what_cannot_be_made_verbatim_is_left_alone() {
        // Relative, device and dot paths: the prefix would change what they mean.
        assert_eq!(verbatim(r"downloads\tv"), None);
        assert_eq!(verbatim(r"D:\downloads\..\tv"), None);
        assert_eq!(
            verbatim(r"\\.\PIPE\rdownloader"),
            Some(PathBuf::from(r"\\.\PIPE\rdownloader"))
        );
        assert_eq!(
            verbatim(r"\\?\D:\already"),
            Some(PathBuf::from(r"\\?\D:\already"))
        );
    }
}
