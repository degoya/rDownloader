//! When a package's files are ready for its post-processing (RD-1190-13).
//!
//! The pipeline asks it to know when to start, and the scheduler asks it to know when a category
//! move has to wait for the pipeline rather than pull its archives out from under it
//! (RD-1290-01): one rule, so the two can never disagree about the moment.

use crate::{DownloadFile, DownloadKind, DownloadState};

/// Whether a package's files are where its post-processing can start (RD-1190-13): at least one
/// completed, and every one completed, stood down as a mirror, or cancelled by somebody.
///
/// A file that waits, failed or is blocked is a part that is missing. Until 1.19 a failed or
/// blocked one counted as settled, so a package whose account ran out of traffic was unpacked
/// with half its volumes, failed for it, and read as finished; it now waits, and the file that
/// completes last starts the pipeline. A Usenet set is the exception and starts once every file
/// settled, failed ones included, as before: PAR2 rebuilds a missing file.
#[must_use]
pub fn ready_for_postprocess(kind: DownloadKind, files: &[DownloadFile]) -> bool {
    let settled = |state: DownloadState| {
        done(state) || matches!(state, DownloadState::Failed | DownloadState::Blocked)
    };
    let any_completed = files.iter().any(|file| {
        matches!(
            file.state,
            DownloadState::Completed | DownloadState::Seeding
        )
    });
    any_completed
        && if kind == DownloadKind::Usenet {
            files.iter().all(|file| settled(file.state))
        } else {
            files.iter().all(|file| done(file.state))
        }
}

/// Whether a package lacks a file it was meant to hold, outside Usenet, where PAR2 may have
/// rebuilt it. Such a package is never `Completed` (RD-1190-13), whatever its steps made of the
/// rest.
#[must_use]
pub fn parts_missing(kind: DownloadKind, files: &[DownloadFile]) -> bool {
    kind != DownloadKind::Usenet && !files.iter().all(|file| done(file.state))
}

/// A file post-processing does not wait for: completed (a seeding torrent is), cancelled by
/// somebody, or a mirror that was never needed.
fn done(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Completed
            | DownloadState::Seeding
            | DownloadState::Cancelled
            | DownloadState::Skipped
    )
}
