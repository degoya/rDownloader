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
const STAGING_PREFIX: &str = ".rd-x";

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
    use super::{create_staging, create_staging_inside};

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
}
