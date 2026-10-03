use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Zip-bomb and file-count boundaries enforced before promotion.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub max_files: u64,
    pub max_uncompressed_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_uncompressed_bytes: 100 * 1024 * 1024 * 1024,
        }
    }
}

/// Successful extraction metrics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExtractionReport {
    pub files: u64,
    pub uncompressed_bytes: u64,
}

pub(crate) fn create_staging(destination: &Path) -> Result<PathBuf> {
    if destination.exists() {
        bail!("extraction destination already exists");
    }
    let parent = destination
        .parent()
        .context("extraction destination has no parent")?;
    std::fs::create_dir_all(parent)?;
    Ok(rd_files::long_path(
        &tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(parent)?
            .keep(),
    ))
}

/// Short on purpose (RD-108-30): the staging directory sits between the package folder and the
/// archive's own tree, so every character of its name is a character the extracted path can no
/// longer use. The old `.rdownloader-extract-` spent 27 of them and pushed a perfectly ordinary
/// release past the Windows limit.
///
/// Public so a walk over a package can leave a staging directory a crash left behind alone.
pub const STAGING_PREFIX: &str = ".rd-x";

/// Staging directory created inside an existing destination (merge mode).
pub(crate) fn create_staging_inside(destination: &Path) -> Result<PathBuf> {
    Ok(rd_files::long_path(
        &tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(destination)?
            .keep(),
    ))
}

/// Moves every entry of `staging` into `destination`; same-named files are replaced and
/// same-named directories merged recursively.
pub(crate) fn merge_tree(staging: &Path, destination: &Path) -> Result<()> {
    for entry in std::fs::read_dir(staging)? {
        let entry = entry?;
        let source = entry.path();
        let target = destination.join(entry.file_name());
        let source_is_dir = entry.file_type()?.is_dir();
        match std::fs::symlink_metadata(&target) {
            Ok(existing) if existing.is_dir() && source_is_dir => {
                merge_tree(&source, &target)?;
                std::fs::remove_dir_all(&source)?;
                continue;
            }
            Ok(existing) if existing.is_dir() => std::fs::remove_dir_all(&target)?,
            Ok(_) => std::fs::remove_file(&target)?,
            Err(_) => {}
        }
        std::fs::rename(&source, &target)
            .with_context(|| format!("move {} into package folder", source.display()))?;
    }
    Ok(())
}

pub(crate) fn promote(staging: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        bail!("extraction destination appeared during processing");
    }
    std::fs::rename(staging, destination).context("promote extracted staging directory")
}

pub(crate) fn validate_tree(root: &Path, limits: ArchiveLimits) -> Result<ExtractionReport> {
    // Both sides of the containment check are brought into the same form (RD-108-30).
    // `dunce::canonicalize` strips the `\\?\` prefix only when the result is a path Windows
    // accepts without it, so on a deep tree the root would come back plain and an entry inside
    // it verbatim - and a file well inside the staging directory would read as having escaped.
    let canonical_root = rd_files::long_path(&dunce::canonicalize(root)?);
    let mut directories = vec![root.to_owned()];
    let mut report = ExtractionReport::default();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                bail!("archive created a symbolic link");
            }
            let canonical = rd_files::long_path(&dunce::canonicalize(entry.path())?);
            if !canonical.starts_with(&canonical_root) {
                bail!("archive output escaped staging directory");
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                if is_hard_linked(&entry.path(), &metadata)? {
                    bail!("archive created a hard link");
                }
                report.files = report.files.saturating_add(1);
                report.uncompressed_bytes = report
                    .uncompressed_bytes
                    .checked_add(metadata.len())
                    .context("archive size overflow")?;
                enforce_limits(report, limits)?;
            }
        }
    }
    Ok(report)
}

/// Whether a regular file has a second name (security review 2026-09-28, finding 5).
///
/// A hard link inside staging to a file outside it passes the canonical-path check - the name
/// is inside, the data is not - and after the promotion the package folder holds a name for,
/// say, a key file in the home directory, readable by whatever reads the download and writable
/// through it. Nothing an unpack writes has a reason to have a second name, so any file with
/// more than one is refused; that includes an archive that links two of its own members, which
/// a download does not ship.
///
/// Unix reads the count from the entry's own metadata. Windows has it only behind
/// `GetFileInformationByHandle` (`MetadataExt::number_of_links` is not stable Rust), so there the
/// file is opened once more, without access rights and without following a reparse point, and
/// asked through `winapi-util` (RD-190-05). Either way a count that cannot be read fails the
/// unpack instead of passing the file.
#[cfg(unix)]
fn is_hard_linked(_path: &Path, metadata: &std::fs::Metadata) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
fn is_hard_linked(path: &Path, _metadata: &std::fs::Metadata) -> Result<bool> {
    use std::os::windows::fs::OpenOptionsExt as _;
    // The handle names the entry itself: a link put in its place after `symlink_metadata` is
    // opened as the link, not as the file it points to, and refused here as one.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u64 = 0x0000_0400;
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .context("open an extracted file to read its link count")?;
    let information =
        winapi_util::file::information(&file).context("read an extracted file's link count")?;
    if information.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        bail!("archive created a symbolic link");
    }
    Ok(information.number_of_links() > 1)
}

pub(crate) fn safe_relative(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut safe = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(value) => safe.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("archive path escapes staging: {name}")
            }
        }
    }
    Ok(safe)
}

pub(crate) fn enforce_limits(report: ExtractionReport, limits: ArchiveLimits) -> Result<()> {
    if report.files > limits.max_files {
        bail!("archive exceeds file-count limit");
    }
    if report.uncompressed_bytes > limits.max_uncompressed_bytes {
        bail!("archive exceeds uncompressed-size limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ArchiveLimits, create_staging, create_staging_inside, validate_tree};

    /// Every character of this name is a character the extracted path can no longer use, and
    /// Windows stops at 260 of them (RD-108-30). The old name spent 27.
    #[test]
    fn the_staging_directory_keeps_a_short_name() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for staging in [
            create_staging_inside(directory.path()).expect("staging inside"),
            create_staging(&directory.path().join("destination")).expect("staging beside"),
        ] {
            let name = staging
                .file_name()
                .and_then(|name| name.to_str())
                .expect("staging name");
            assert!(
                name.len() <= 12,
                "the staging directory name {name} is {} characters",
                name.len()
            );
        }
    }

    /// Security review 2026-09-28, finding 5: a hard link inside staging to a file outside it
    /// has a canonical path inside staging, so only the link count gives it away. Without the
    /// link-count check the tree validates and the foreign file would be promoted into the
    /// package. It runs on Windows too (RD-190-05), where the count comes from the file handle.
    #[test]
    fn a_hard_link_to_a_file_outside_staging_is_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let outside = directory.path().join("id_ed25519");
        std::fs::write(&outside, b"private key").expect("file outside staging");
        let staging = directory.path().join("staging");
        std::fs::create_dir_all(staging.join("nested")).expect("staging");
        std::fs::write(staging.join("payload.bin"), b"payload").expect("ordinary member");
        assert!(validate_tree(&staging, ArchiveLimits::default()).is_ok());

        std::fs::hard_link(&outside, staging.join("nested/innocent.txt")).expect("hard link");
        let error = validate_tree(&staging, ArchiveLimits::default())
            .expect_err("a hard link out of staging must not validate");
        assert!(error.to_string().contains("hard link"), "{error}");
    }

    /// A symbolic link out of staging - to a file, to a directory, or to nothing yet - is
    /// refused by the tree walk before anything is promoted. The canonical-path check behind it
    /// is a second line: on a local file system nothing but a link makes an entry's canonical
    /// path leave the tree, so this test is its proof too.
    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_out_of_staging_is_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let outside_file = directory.path().join("authorized_keys");
        std::fs::write(&outside_file, b"ssh-ed25519 AAAA").expect("file outside staging");
        let outside_directory = directory.path().join("autostart");
        std::fs::create_dir_all(&outside_directory).expect("directory outside staging");
        for (name, target) in [
            ("file", outside_file.clone()),
            ("directory", outside_directory.clone()),
            ("dangling", directory.path().join("not-there-yet")),
        ] {
            let staging = directory.path().join(format!("staging-{name}"));
            std::fs::create_dir_all(staging.join("nested")).expect("staging");
            std::os::unix::fs::symlink(&target, staging.join("nested/link")).expect("link");
            let error = validate_tree(&staging, ArchiveLimits::default())
                .expect_err("a link out of staging must not validate");
            assert!(
                error.to_string().contains("symbolic link"),
                "{name}: {error}"
            );
        }
    }

    /// A junction is a reparse point like a symbolic link, and the standard library's file type
    /// counts it as one (a name-surrogate tag), so the same refusal covers it (RD-190-05).
    /// `mklink /J` needs no privilege, unlike a symbolic link on Windows.
    #[cfg(windows)]
    #[test]
    fn a_junction_out_of_staging_is_refused() {
        use std::os::windows::process::CommandExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        let outside_directory = directory.path().join("autostart");
        std::fs::create_dir_all(&outside_directory).expect("directory outside staging");
        let staging = directory.path().join("staging");
        std::fs::create_dir_all(staging.join("nested")).expect("staging");
        let link = staging.join("nested").join("link");
        let status = std::process::Command::new("cmd")
            .raw_arg(format!(
                "/C mklink /J \"{}\" \"{}\"",
                link.display(),
                outside_directory.display()
            ))
            .status()
            .expect("mklink");
        assert!(status.success(), "mklink /J: {status}");
        let error = validate_tree(&staging, ArchiveLimits::default())
            .expect_err("a junction out of staging must not validate");
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }

    /// The limits hold for a tree an external tool wrote, where nothing counted while it was
    /// written: one file too many, or one byte too many, fails the validation.
    #[test]
    fn a_tree_over_either_limit_is_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let staging = directory.path().join("staging");
        std::fs::create_dir_all(staging.join("nested")).expect("staging");
        std::fs::write(staging.join("a.bin"), vec![0_u8; 600]).expect("file");
        std::fs::write(staging.join("nested/b.bin"), vec![0_u8; 600]).expect("file");
        let at = |max_files, max_uncompressed_bytes| ArchiveLimits {
            max_files,
            max_uncompressed_bytes,
        };

        let report = validate_tree(&staging, at(2, 1_200)).expect("exactly at both limits");
        assert_eq!((report.files, report.uncompressed_bytes), (2, 1_200));
        let count = validate_tree(&staging, at(1, u64::MAX)).expect_err("two files over one");
        assert!(count.to_string().contains("file-count limit"), "{count}");
        let size = validate_tree(&staging, at(10, 1_199)).expect_err("one byte over");
        assert!(
            size.to_string().contains("uncompressed-size limit"),
            "{size}"
        );
    }
}
