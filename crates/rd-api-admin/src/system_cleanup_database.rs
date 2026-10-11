//! The database's part of the clean-up (RD-1240-35): the subscription archive's compaction and
//! the free pages handed back to the file system.
//!
//! A skipped or dismissed subscription item older than `subscription_item_retention_days`
//! (default 30, 0 = whole for good) keeps only its key, which is what the poll recognises it by
//! (`rd_db::Database::compact_subscription_items`). The pages that frees, and those the event
//! retention frees, stay in the file until they are handed back: in a file with
//! `auto_vacuum = INCREMENTAL` (every one this build creates) by `PRAGMA incremental_vacuum`, at
//! every pass. A file created before needs one `VACUUM` to become so: a rewrite of the whole
//! file, so only when somebody asks for it ("Clean up now"), only with room for a second copy
//! beside the file, and only while nothing downloads — the rewrite holds the serialized writer
//! for its whole length, and every progress checkpoint of a running transfer would wait behind
//! it. Each refusal is a code in the answer; the rest of the clean-up runs either way.

use std::path::Path;

use chrono::Utc;
use rd_db::SubscriptionItemRetention;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// Refused: something downloads, verifies, unpacks or records.
pub const REWRITE_BUSY: &str = "system.cleanup_rewrite_busy";
/// Refused: the data directory has not room for a second copy of the database.
pub const REWRITE_NO_SPACE: &str = "system.cleanup_rewrite_no_space";
/// The rewrite was started and failed; the file is as it was.
pub const REWRITE_FAILED: &str = "system.cleanup_rewrite_failed";

/// Which pass this is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pass {
    /// Measures; changes nothing. Says what [`Pass::Requested`] would do.
    Preview,
    /// After the start and once a day: compacts and hands free pages back, never rewrites.
    Automatic,
    /// "Clean up now": also rewrites a file that is not incremental yet.
    Requested,
}

/// The database file and the stores in it that grow with use.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
pub struct DatabaseCleanup {
    /// The file, once the WAL is checkpointed; after a clean-up its new size.
    pub file_bytes: u64,
    /// Free pages inside the file, waiting to be reused or handed back.
    pub free_bytes: u64,
    /// Whether free pages go back without a rewrite (`auto_vacuum = INCREMENTAL`). A file
    /// created before 1.24 is not, until its first rewrite.
    pub incremental: bool,
    /// The persisted events and their size, indexes included. Kept 30 days; the change notices
    /// of Usenet and the LinkGrabber are broadcast only.
    pub event_rows: u64,
    pub event_bytes: u64,
    /// The subscription archive's full rows and their size, indexes included.
    pub item_rows: u64,
    pub item_bytes: u64,
    /// Keys of compacted items: what the poll still recognises them by.
    pub item_key_rows: u64,
    /// `subscription_item_retention_days`: how long a skipped or dismissed item keeps its full
    /// row; 0 for good.
    pub item_retention_days: u32,
    /// In the preview the skipped or dismissed items past the retention and an estimate of
    /// their bytes; in a clean-up's answer the items it compacted.
    pub compactable_items: u64,
    pub compactable_bytes: u64,
    /// In the preview about how much smaller "Clean up now" makes the file; in a clean-up's
    /// answer how much smaller it became.
    pub removable_bytes: u64,
    /// Why the file is not (or would not be) rewritten although it is not incremental:
    /// `system.cleanup_rewrite_busy`, `system.cleanup_rewrite_no_space` or
    /// `system.cleanup_rewrite_failed`. Empty when it is rewritten or need not be.
    pub rewrite_refused: Option<String>,
}

/// The database's pass; `data` is the folder the file lives in.
pub(crate) async fn clean_up(
    state: &AppState,
    data: &Path,
    pass: Pass,
) -> Result<DatabaseCleanup, ApiError> {
    let retention: SubscriptionItemRetention = state.database.service_settings_or_default().await?;
    let before = retention.compact_before(Utc::now());
    let measured = state.database.database_storage(before).await?;
    let mut summary = DatabaseCleanup {
        file_bytes: measured.file_bytes,
        free_bytes: measured.free_bytes,
        incremental: measured.incremental,
        event_rows: measured.event_rows,
        event_bytes: measured.event_bytes,
        item_rows: measured.item_rows,
        item_bytes: measured.item_bytes,
        item_key_rows: measured.item_key_rows,
        item_retention_days: retention.days(),
        compactable_items: measured.compactable_items,
        compactable_bytes: measured.compactable_bytes,
        removable_bytes: 0,
        rewrite_refused: None,
    };
    let rewrite = pass != Pass::Automatic && !measured.incremental;
    if rewrite {
        summary.rewrite_refused = rewrite_refusal(state, data, measured.file_bytes).await;
    }
    let shrinks = measured.incremental || (rewrite && summary.rewrite_refused.is_none());
    if pass == Pass::Preview {
        if shrinks {
            summary.removable_bytes = measured.free_bytes + measured.compactable_bytes;
        }
        return Ok(summary);
    }

    if let Some(before) = before {
        summary.compactable_items = state.database.compact_subscription_items(before).await?;
    }
    if measured.incremental {
        state.database.reclaim_free_pages().await?;
    } else if rewrite
        && summary.rewrite_refused.is_none()
        && let Err(error) = state.database.rewrite().await
    {
        tracing::warn!(%error, "the database could not be rewritten; it stays as it was");
        summary.rewrite_refused = Some(REWRITE_FAILED.to_owned());
    }
    let after = state.database.database_storage(None).await?;
    summary.removable_bytes = measured.file_bytes.saturating_sub(after.file_bytes);
    summary.compactable_bytes = measured.item_bytes.saturating_sub(after.item_bytes);
    summary.file_bytes = after.file_bytes;
    summary.free_bytes = after.free_bytes;
    summary.incremental = after.incremental;
    summary.event_rows = after.event_rows;
    summary.event_bytes = after.event_bytes;
    summary.item_rows = after.item_rows;
    summary.item_bytes = after.item_bytes;
    summary.item_key_rows = after.item_key_rows;
    Ok(summary)
}

/// Why a rewrite must not start now, if it must not: something runs that it would hold up, or
/// the data directory has not room for the copy SQLite builds and the WAL it writes it through
/// — up to twice the file — beside the reserve a restore keeps free too. A file system that
/// cannot say how much is free lets it start; SQLite then fails on its own and leaves the file.
async fn rewrite_refusal(state: &AppState, data: &Path, file_bytes: u64) -> Option<String> {
    if crate::update_auto_install::busy(state).await {
        return Some(REWRITE_BUSY.to_owned());
    }
    match rd_files::available_space(data).await {
        Ok(available) if !room_for_rewrite(available, file_bytes) => {
            Some(REWRITE_NO_SPACE.to_owned())
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(%error, "free space unknown; the database rewrite goes on");
            None
        }
    }
}

/// Whether `available` bytes hold the copy and the WAL of a `file_bytes` file and the reserve.
fn room_for_rewrite(available: u64, file_bytes: u64) -> bool {
    crate::restore_room::fits(available, file_bytes.saturating_mul(2))
}

#[cfg(test)]
mod tests {
    use super::room_for_rewrite;
    use crate::restore_room::RESERVE_BYTES;

    #[test]
    fn a_rewrite_needs_twice_the_file_beside_the_reserve() {
        let file = 500 << 20;
        assert!(room_for_rewrite(RESERVE_BYTES + 2 * file, file));
        assert!(!room_for_rewrite(RESERVE_BYTES + 2 * file - 1, file));
        assert!(!room_for_rewrite(2 * file, file), "the reserve stays free");
        assert!(
            !room_for_rewrite(u64::MAX - 1, u64::MAX),
            "no overflow lets it through"
        );
    }
}
