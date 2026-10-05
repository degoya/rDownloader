//! Giving up on a Usenet set as soon as it cannot be repaired (RD-1100-02).
//!
//! The arithmetic is [`crate::health`]; this is where the runner asks it and acts on the
//! answer. It asks after every file that finished with holes or came back with nothing, which
//! is the moment the set's damage grows - a missing article is answered in milliseconds, so a
//! set whose articles are gone gets there quickly. The inputs are what the database records:
//! the segments every server refused, the rows' states and names. A restart in the middle
//! therefore reaches the same verdict on the same rows again.
//!
//! Acting on it has two halves. The rows that wait fail in one write
//! ([`rd_db::Database::fail_hopeless_usenet_package`]); the files of the set that are running
//! right now are this runner's own, and [`Verdicts`] stops them through a token their
//! attempts listen to. What was downloaded stays where it is.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use anyhow::Result;
use rd_core::{
    DownloadFile, DownloadId, DownloadState, Failure, FailureKind, NzbFileStatus, NzbImportId,
    NzbSegmentState, PackageId,
};
use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::health::{SetFile, Shortfall, Standing, shortfall};

/// The verdicts this runner has reached, per NZB import, while files of it are running.
#[derive(Default)]
pub(crate) struct Verdicts(Mutex<HashMap<NzbImportId, Watch>>);

/// The running files of one import and what they listen to.
struct Watch {
    running: usize,
    /// Cancelled when the set is given up; every running attempt of the import listens.
    abort: CancellationToken,
    verdict: Option<Failure>,
}

/// One running attempt's place in [`Verdicts`]; leaving it is dropping it.
pub(crate) struct Enrolment<'a> {
    verdicts: &'a Verdicts,
    import_id: NzbImportId,
    abort: CancellationToken,
}

impl Verdicts {
    /// Puts an attempt of `import_id` on the books until the returned enrolment is dropped.
    pub(crate) fn enrol(&self, import_id: NzbImportId) -> Enrolment<'_> {
        let mut watches = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let watch = watches.entry(import_id).or_insert_with(|| Watch {
            running: 0,
            abort: CancellationToken::new(),
            verdict: None,
        });
        watch.running += 1;
        Enrolment {
            verdicts: self,
            import_id,
            abort: watch.abort.clone(),
        }
    }

    /// The verdict reached on `import_id` while files of it were running, if any.
    pub(crate) fn verdict(&self, import_id: NzbImportId) -> Option<Failure> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&import_id)
            .and_then(|watch| watch.verdict.clone())
    }

    /// Records the verdict and stops every running attempt of the import.
    ///
    /// `false` when a verdict was recorded already: two files that finish at the same moment
    /// may both reach it, and only the first gives the set up.
    pub(crate) fn condemn(&self, import_id: NzbImportId, failure: &Failure) -> bool {
        let watches = &mut *self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(watch) = watches.get_mut(&import_id) else {
            return true;
        };
        if watch.verdict.is_some() {
            return false;
        }
        watch.verdict = Some(failure.clone());
        watch.abort.cancel();
        true
    }
}

impl Enrolment<'_> {
    /// Cancelled when the set this attempt belongs to is given up.
    pub(crate) fn abort(&self) -> &CancellationToken {
        &self.abort
    }
}

impl Drop for Enrolment<'_> {
    fn drop(&mut self) {
        let watches = &mut *self
            .verdicts
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(watch) = watches.get_mut(&self.import_id) {
            watch.running = watch.running.saturating_sub(1);
            if watch.running == 0 {
                watches.remove(&self.import_id);
            }
        }
    }
}

/// How the file whose end prompted the question ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Ending {
    /// Assembled, with zeros where the articles no server had belong.
    Holes,
    /// Not one article was on any server.
    Lost,
}

/// The failure a set beyond repair ends with, carrying both numbers that decide it.
pub(crate) fn hopeless_failure(shortfall: Shortfall) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        rd_db::USENET_JOB_HOPELESS,
        format!(
            "{} PAR2 blocks are missing and at most {} are available to repair them; the rest of the set is not downloaded",
            shortfall.missing_blocks, shortfall.available_blocks
        ),
    )
    .with_param("missing_blocks", shortfall.missing_blocks)
    .with_param("available_blocks", shortfall.available_blocks)
}

/// Whether the set `current` belongs to is beyond repair now that `current` ended as `ending`.
///
/// `None` with the setting switched off, for a set that can still be repaired, and for one
/// that does not say enough to tell.
pub(crate) async fn judge(
    database: &Database,
    package_id: PackageId,
    import_id: NzbImportId,
    current: DownloadId,
    ending: Ending,
) -> Result<Option<Failure>> {
    // On unless switched off, like SABnzbd's switch; a missing or unreadable field reads as on.
    let enabled = database
        .service_setting_field::<bool>("fail_hopeless_jobs")
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(None);
    }
    let nzb_files = database.list_nzb_files(import_id).await?;
    let rows = database.downloads_for_package(package_id).await?;
    let set: Vec<SetFile<'_>> = rows
        .iter()
        .filter_map(|row| {
            let nzb = nzb_files
                .iter()
                .find(|file| Some(file.id) == row.nzb_file_id)?;
            let standing = if row.id == current {
                match ending {
                    Ending::Holes => Standing::Finished {
                        missing_bytes: refused_bytes(nzb),
                    },
                    Ending::Lost => Standing::Lost,
                }
            } else {
                standing_of(row, nzb)
            };
            Some(SetFile {
                name: &row.file_name,
                recovery: row.recovery,
                nzb_bytes: nzb.total_bytes.get(),
                standing,
            })
        })
        .collect();
    Ok(shortfall(&set).map(hopeless_failure))
}

/// What a row other than the one just finished says about its articles.
///
/// Only an ended attempt says anything: a row that is running or waiting may still find every
/// article, and its `Failed` segments from an earlier attempt are fetched again.
fn standing_of(row: &DownloadFile, nzb: &NzbFileStatus) -> Standing {
    let code = row
        .last_error
        .as_ref()
        .and_then(|failure| failure.code.as_deref());
    match row.state {
        DownloadState::Completed => Standing::Finished {
            missing_bytes: refused_bytes(nzb),
        },
        DownloadState::Verifying if code == Some(rd_db::USENET_AWAITING_PAR2) => {
            Standing::Finished {
                missing_bytes: refused_bytes(nzb),
            }
        }
        DownloadState::Failed
            if matches!(
                code,
                Some("usenet.all_segments_missing" | "usenet.recovery_unavailable")
            ) =>
        {
            Standing::Lost
        }
        _ => Standing::Open,
    }
}

/// The NZB weight of the articles every server refused.
fn refused_bytes(nzb: &NzbFileStatus) -> u64 {
    nzb.segments
        .iter()
        .filter(|segment| segment.state == NzbSegmentState::Failed)
        .map(|segment| segment.bytes.get())
        .sum()
}

/// Gives up on the set: the verdict for the attempts still running, then the write for the
/// rows that wait.
///
/// The order is what makes a stop between the two harmless. Nothing is persisted before the
/// write, so a restart finds the rows as they were, the file that prompted the verdict
/// downloads its refused articles again, is refused again, and the same verdict follows.
pub(crate) async fn give_up(
    database: &Database,
    verdicts: &Verdicts,
    package_id: PackageId,
    import_id: NzbImportId,
    failure: &Failure,
) -> Result<()> {
    if !verdicts.condemn(import_id, failure) {
        return Ok(());
    }
    tracing::warn!(
        package_id = %package_id,
        missing_blocks = failure.params.get("missing_blocks").map(String::as_str),
        available_blocks = failure.params.get("available_blocks").map(String::as_str),
        "usenet set is beyond repair; the rest of it is not downloaded"
    );
    rd_core::failpoint!("usenet.before_hopeless_abort", || {
        anyhow::anyhow!("crash point: usenet.before_hopeless_abort")
    });
    database
        .fail_hopeless_usenet_package(package_id, failure.clone())
        .await
}
