use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use rd_core::DownloadId;

use crate::StorageRoot;

/// `ENOSPC` on Unix, `ERROR_DISK_FULL` on Windows.
#[cfg(unix)]
const NO_SPACE: i32 = 28;
#[cfg(windows)]
const NO_SPACE: i32 = 112;

/// Directory inside a storage root that holds the files of transfers still running.
///
/// Frozen, together with the `<id>.part` spelling below: a partial file an earlier version
/// left on disk has to be found again after an upgrade, and a renamed staging directory would
/// silently restart every interrupted download from zero.
const STAGING_DIRECTORY: &str = ".rdownloader";

/// Creates the staging directory inside `root` and returns the partial file's path for `id`.
///
/// The one place this layout is built. It was four lines in each of `rd-ftp`, `rd-sftp`,
/// `rd-transfer-file` and `rd-plugin-transfer`, which is three chances for the next change to
/// reach only some of them.
pub async fn part_path(root: &StorageRoot, id: DownloadId) -> Result<PathBuf> {
    let staging = root.resolve(Path::new(STAGING_DIRECTORY))?;
    tokio::fs::create_dir_all(&staging)
        .await
        .with_context(|| format!("create staging directory {}", staging.display()))?;
    Ok(staging.join(format!("{id}.part")))
}

/// Bytes already on disk for a partial file, or zero when nothing was written yet.
///
/// A missing file and one whose metadata cannot be read are the same answer: there is nothing
/// to resume from. Reporting a length that could not be confirmed would let a transfer append
/// behind bytes that are not there.
pub async fn existing_bytes(part_path: &Path) -> u64 {
    tokio::fs::metadata(part_path)
        .await
        .map(|meta| meta.len())
        .unwrap_or_default()
}

/// Preallocated partial file supporting positioned concurrent writes.
#[derive(Clone)]
pub struct PartFile {
    path: PathBuf,
    file: Arc<std::fs::File>,
}

impl PartFile {
    /// Creates or opens a partial file and applies best-effort preallocation.
    pub async fn open(path: PathBuf, expected_length: Option<u64>) -> Result<Self> {
        let open_path = path.clone();
        let file = tokio::task::spawn_blocking(move || {
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&open_path)
                .with_context(|| format!("open partial file {}", open_path.display()))?;
            if let Some(length) = expected_length {
                // Preallocation is best effort: filesystems that cannot size a file up front
                // simply keep it sparse. Running out of space is a different matter and must
                // surface, or the transfer would fail later with a confusing write error.
                if let Err(error) = file.set_len(length) {
                    if error.raw_os_error() == Some(NO_SPACE) {
                        return Err(anyhow::Error::new(error))
                            .context("preallocate partial file: no space left on device");
                    }
                    tracing::debug!(%error, "partial file could not be preallocated");
                }
            }
            Ok::<_, anyhow::Error>(file)
        })
        .await??;
        Ok(Self {
            path,
            file: Arc::new(file),
        })
    }

    /// Writes the full buffer at an absolute file offset.
    pub async fn write_at(&self, offset: u64, bytes: Vec<u8>) -> Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || write_all_at(&file, offset, &bytes)).await?
    }

    /// Flushes dirty content before a checkpoint is persisted.
    pub async fn sync_data(&self) -> Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || file.sync_data().context("sync partial file")).await?
    }

    /// Atomically promotes the verified partial file.
    ///
    /// Precondition: this must be the last live handle on the file, and the call checks it
    /// rather than assuming it. `PartFile` is `Clone` and the clone shares one
    /// `Arc<std::fs::File>`, so dropping this one closes nothing while a second holder is
    /// alive — a `TransferTarget` handed to a plugin guest, or a `write_at` still running.
    /// Renaming underneath such a holder fails outright on Windows with a sharing violation,
    /// and on Unix it succeeds while the other handle keeps writing into a file that has
    /// already been published. Refusing is the only answer that is the same on both.
    pub async fn finalize(self, destination: &Path) -> Result<()> {
        let Self { path, file } = self;
        let Some(file) = Arc::into_inner(file) else {
            anyhow::bail!(
                "refusing to promote {}: the partial file is still open elsewhere",
                path.display()
            );
        };
        // Sync and close in the same blocking task. Dropping the last owner is what closes
        // the handle, and the rename below needs that to have happened already, so the drop
        // is spelled out rather than left to the end of the closure.
        let closed = tokio::task::spawn_blocking(move || {
            let synced = file.sync_data().context("sync partial file");
            drop(file);
            synced
        });
        closed.await??;
        tokio::fs::rename(&path, destination)
            .await
            .with_context(|| format!("rename {} to {}", path.display(), destination.display()))
    }

    /// Returns the path of the partial file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn write_all_at(file: &std::fs::File, mut offset: u64, mut bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;
    while !bytes.is_empty() {
        let written = file.write_at(bytes, offset)?;
        if written == 0 {
            anyhow::bail!("positioned write returned zero bytes");
        }
        offset += written as u64;
        bytes = &bytes[written..];
    }
    Ok(())
}

#[cfg(windows)]
fn write_all_at(file: &std::fs::File, mut offset: u64, mut bytes: &[u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        let written = file.seek_write(bytes, offset)?;
        if written == 0 {
            anyhow::bail!("positioned write returned zero bytes");
        }
        offset += written as u64;
        bytes = &bytes[written..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rd_core::{DownloadId, StorageRootId};

    use super::{PartFile, StorageRoot, existing_bytes, part_path};

    #[tokio::test]
    async fn refuses_to_promote_while_a_second_holder_is_alive() {
        let directory = tempfile::tempdir().expect("tempdir");
        let part = PartFile::open(directory.path().join("payload.part"), None)
            .await
            .expect("open");
        part.write_at(0, b"payload".to_vec()).await.expect("write");

        // This is the shape `rd-plugin-transfer` had: the clone goes to the guest through
        // `TransferTarget` while the runner keeps the original and promotes it.
        let guest = part.clone();
        let error = part
            .finalize(&directory.path().join("payload"))
            .await
            .expect_err("promoting behind a live handle must be refused");
        assert!(
            error.to_string().contains("still open elsewhere"),
            "unexpected error: {error}"
        );
        assert!(
            !directory.path().join("payload").exists(),
            "the file must not be published while another handle can still write to it"
        );
        drop(guest);
    }

    #[tokio::test]
    async fn promotes_once_it_is_the_last_holder() {
        let directory = tempfile::tempdir().expect("tempdir");
        let part_file = directory.path().join("payload.part");
        let part = PartFile::open(part_file.clone(), None).await.expect("open");
        part.write_at(0, b"payload".to_vec()).await.expect("write");

        let guest = part.clone();
        drop(guest);

        let destination = directory.path().join("payload");
        part.finalize(&destination).await.expect("finalize");
        assert!(!part_file.exists());
        assert_eq!(
            tokio::fs::read(&destination).await.expect("read"),
            b"payload".to_vec()
        );
    }

    #[tokio::test]
    async fn the_staging_layout_is_the_one_on_disk_today() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = StorageRoot::create(
            StorageRootId::new(),
            "downloads".to_owned(),
            directory.path().to_owned(),
        )
        .await
        .expect("root");
        let id = DownloadId::new();

        let path = part_path(&root, id).await.expect("part path");

        // Spelled out rather than derived from the same constant: this is the layout an
        // upgrade has to keep finding, so the test has to fail when it is changed.
        assert_eq!(
            path,
            root.path().join(".rdownloader").join(format!("{id}.part"))
        );
        assert!(path.parent().expect("parent").is_dir());
        assert_eq!(existing_bytes(&path).await, 0, "nothing written yet");

        tokio::fs::write(&path, b"1234").await.expect("write");
        assert_eq!(existing_bytes(&path).await, 4);
    }
}
