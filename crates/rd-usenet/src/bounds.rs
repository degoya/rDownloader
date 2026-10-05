//! What a yEnc header may make the assembly write (RD-1101-16, audit S6).
//!
//! An article's `=ybegin size=` is the one number the assembly sizes a file by, and the
//! poster writes it. Before this it bounded nothing: a file with a missing article was grown
//! to that size and every byte no article covered was zero-filled, so one article announcing
//! a terabyte filled the disk - past the capacity check, which admitted the file by what the
//! NZB lists, and past the capacity supervision, which stops transfers but not a zero-fill.
//! Now the size is held against the NZB's own count before anything is written, the
//! zero-fill against the free space before it starts, and a stop reaches the fill as well.

use std::path::Path;

use anyhow::Result;
use rd_core::{Failure, FailureKind, NzbFileStatus};
use rd_db::Database;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// How long a file waits before it asks again when its gaps did not fit on the disk.
const SPACE_RETRY_SECONDS: u64 = 15 * 60;

/// The largest `size=` a yEnc header may announce for a file the NZB lists at `listed` bytes.
///
/// The NZB counts each article as it was posted: yEnc-encoded, which makes the data one to two
/// per cent larger, plus the article's own lines, so an honest header announces less than the
/// NZB lists. An eighth on top and one mebibyte for the smallest files absorb an NZB that
/// counted decoded bytes or rounded them.
pub(crate) fn declared_size_limit(listed: u64) -> u64 {
    listed
        .saturating_add(listed / 8)
        .saturating_add(1024 * 1024)
}

/// Refuses a file whose articles announce a size the NZB does not account for.
pub(crate) fn check_declared_size(declared: u64, file: &NzbFileStatus) -> Result<()> {
    let listed = file.total_bytes.get();
    if declared <= declared_size_limit(listed) {
        return Ok(());
    }
    Err(Failure::coded(
        FailureKind::Permanent,
        "usenet.declared_size_implausible",
        "The articles announce a file far larger than the NZB lists",
    )
    .with_param("declared", declared)
    .with_param("listed", listed)
    .into())
}

/// What filling the gaps of a file's missing articles needs besides the file.
pub(crate) struct Gaps<'a> {
    pub database: &'a Database,
    /// Where the part file lives, which is whose free space counts.
    pub staging: &'a Path,
    pub shutdown: &'a CancellationToken,
}

impl Gaps<'_> {
    /// Grows `output` to `size` and writes zeros over `holes`, 1-based and inclusive; `false`
    /// when a stop came first.
    ///
    /// Written rather than left to the file system: a part file resumed after a crash can hold
    /// the remains of an article interrupted mid-write, and those bytes are not zeros.
    pub(crate) async fn fill(
        &self,
        output: &mut tokio::fs::File,
        size: u64,
        holes: &[(u64, u64)],
    ) -> Result<bool> {
        static ZEROS: [u8; 64 * 1024] = [0; 64 * 1024];
        let bytes = holes.iter().map(|(begin, end)| end - begin + 1).sum();
        self.ensure_room(bytes).await?;
        output.set_len(size).await?;
        for (begin, end) in holes {
            output.seek(std::io::SeekFrom::Start(begin - 1)).await?;
            let mut remaining = end - begin + 1;
            while remaining > 0 {
                if self.shutdown.is_cancelled() {
                    return Ok(false);
                }
                let chunk = usize::try_from(remaining.min(ZEROS.len() as u64))?;
                output.write_all(&ZEROS[..chunk]).await?;
                remaining -= chunk as u64;
            }
        }
        Ok(true)
    }

    /// Refuses a fill of `bytes` the disk cannot take with the reserve left over.
    ///
    /// The reserve is the global one: the per-root thresholds are the scheduler's, and the
    /// scheduler blocks a root that falls below its own anyway. A disk whose free space cannot
    /// be read is not refused here; a write that really does not fit fails with its own error.
    async fn ensure_room(&self, bytes: u64) -> Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        let storage: rd_core::StorageSettings = self.database.service_settings_or_default().await?;
        let required = bytes.saturating_add(storage.storage_minimum_free_bytes.get());
        let free = match rd_files::available_space(self.staging).await {
            Ok(free) => free,
            Err(error) => {
                tracing::debug!(%error, "free space unknown; the gaps are filled without a check");
                return Ok(());
            }
        };
        if free >= required {
            return Ok(());
        }
        Err(Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: Some(SPACE_RETRY_SECONDS),
            },
            "usenet.gap_fill_no_space",
            "Not enough free space to fill the gaps of the missing articles",
        )
        .with_param("required", required)
        .with_param("free", free)
        .into())
    }
}
