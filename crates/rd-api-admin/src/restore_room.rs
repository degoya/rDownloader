//! Whether the data directory has room for what a restore writes (RD-1190-22).
//!
//! An upload may grow to a tebibyte and the unpack writes about as much again, into the folder
//! the database lives in. A full disk there stops the database's own writes, so each chunk and
//! the unpack are refused while they would leave less than [`RESERVE_BYTES`] free.

use std::path::Path;

use crate::ApiError;

/// What the data directory keeps free beside a restore's files.
pub(crate) const RESERVE_BYTES: u64 = 1 << 30;

/// Whether `needed` bytes fit into `available` and leave the reserve.
pub(crate) fn fits(available: u64, needed: u64) -> bool {
    available >= RESERVE_BYTES.saturating_add(needed)
}

/// `409 backup.restore_no_space` unless `needed` bytes fit below `folder`. A file system that
/// cannot say how much is free is let through, with a warning: the write then fails on its own.
pub(crate) async fn require_room(folder: &Path, needed: u64) -> Result<(), ApiError> {
    let available = match rd_files::available_space(folder).await {
        Ok(available) => available,
        Err(error) => {
            tracing::warn!(%error, folder = %folder.display(), "free space unknown; the restore goes on");
            return Ok(());
        }
    };
    if fits(available, needed) {
        return Ok(());
    }
    Err(ApiError::conflict(
        "backup.restore_no_space",
        "The data directory has not enough free space for the restore",
    )
    .with_param("needed", needed)
    .with_param("available", available))
}

#[cfg(test)]
mod tests {
    use super::{RESERVE_BYTES, fits};

    #[test]
    fn the_reserve_stays_free() {
        assert!(fits(RESERVE_BYTES + 10, 10));
        assert!(!fits(RESERVE_BYTES + 10, 11));
        assert!(!fits(RESERVE_BYTES / 2, 0));
        assert!(fits(u64::MAX, 1 << 40));
    }
}
