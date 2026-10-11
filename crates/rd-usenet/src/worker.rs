//! NZB file assembly: segment download, resume, gap filling and final placement.

use std::{ops::ControlFlow, path::PathBuf};

use anyhow::{Context, Result, bail};
use futures_util::{FutureExt, StreamExt};
use rd_core::{Failure, FailureKind, NzbFileStatus, NzbSegmentState};
use rd_db::Database;
use rd_files::{collision_free_path, sanitize_file_name};
use tokio_util::sync::CancellationToken;

use crate::{NntpPool, checkpoints::PendingCheckpoints, parallel::OpenArticles, pool::FetchError};

#[path = "worker_assembly.rs"]
mod assembly;

use assembly::Assembly;

pub(crate) async fn recovered_file_path(
    file: &NzbFileStatus,
    destination: &std::path::Path,
) -> Result<Option<PathBuf>> {
    if !file
        .segments
        .iter()
        .all(|segment| segment.state == NzbSegmentState::Completed)
    {
        return Ok(None);
    }
    let Some(stored) = file.output_path.as_deref() else {
        return Ok(None);
    };
    let canonical = match dunce::canonicalize(stored) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !canonical.starts_with(destination) || !canonical.is_file() {
        return Ok(None);
    }
    crate::assembly_resume::validates_completed_file(file, &canonical)
        .await
        .map(|valid| valid.then_some(canonical))
}

/// The failure for a file whose every segment was gone from every server.
///
/// Recovery data gets its own code. An expired `vol…par2` volume is routine on Usenet and it
/// is not a defect by itself — a package whose payload arrived complete never asked for it —
/// whereas a payload file that is gone is exactly the failure the old code names. Splitting
/// the two here is what lets the queue stop reporting the first as an error (RD-107-10).
pub(crate) fn missing_everything_failure(subject: &str) -> Failure {
    let name = rd_collector::subject_file_name(subject);
    if rd_core::is_recovery_volume(name.as_deref().unwrap_or(subject)) {
        return Failure::coded(
            FailureKind::Permanent,
            "usenet.recovery_unavailable",
            "The PAR2 recovery volume is no longer available on any server",
        );
    }
    Failure::coded(
        FailureKind::Permanent,
        "usenet.all_segments_missing",
        "All segments of the file are missing on every server",
    )
}

/// The failure for a server that could not answer, however often it was asked.
///
/// Retryable on purpose: the article may well be there, and the next attempt in a few
/// minutes finds it. Everything written so far stays on disk, so the retry resumes rather
/// than starting the file again (RD-108-29).
fn server_unavailable_failure(detail: &str) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: Some(SERVER_RETRY_SECONDS),
        },
        "usenet.server_unavailable",
        "The Usenet server could not deliver the article",
    )
    .with_param("detail", detail)
}

/// How long the queue waits before it asks the server again.
const SERVER_RETRY_SECONDS: u64 = 60;

/// [`download_file_counted`] for a file nobody counts: the tests and the bench.
#[cfg(test)]
pub(crate) async fn download_file(
    database: &Database,
    pool: &NntpPool,
    shutdown: &CancellationToken,
    nzb_file: &NzbFileStatus,
    staging: &std::path::Path,
    destination: &std::path::Path,
    limits: &rd_scheduler::RunLimits,
) -> Result<FileOutcome> {
    download_file_counted(
        database,
        pool,
        shutdown,
        nzb_file,
        staging,
        destination,
        limits,
        &OpenArticles::untracked(),
    )
    .await
}

/// `download_file`, telling `open` about every article that is answered, so the runner
/// knows how much work its running files still hold (RD-130-22).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_file_counted(
    database: &Database,
    pool: &NntpPool,
    shutdown: &CancellationToken,
    nzb_file: &NzbFileStatus,
    staging: &std::path::Path,
    destination: &std::path::Path,
    limits: &rd_scheduler::RunLimits,
    open: &OpenArticles,
) -> Result<FileOutcome> {
    let part_path = staging.join(format!("{}.part", nzb_file.id));
    if nzb_file.segments.is_empty() {
        bail!("NZB file contains no segments");
    }
    let (mut assembly, segments) = Assembly::resume(database, nzb_file, &part_path).await?;
    let single_segment = assembly.single_segment;
    // No `Downloading` mark before the request any more (RD-130-22). It was a writer
    // transaction per article that nothing depends on: the resume trusts only checkpoints,
    // the start-up recovery turns a leftover mark back into `Queued` anyway, and the one
    // thing it carried - the attempt count - now travels with the checkpoint.
    let fetches = futures_util::stream::iter(segments)
        .map(|segment| fetch_segment(database, pool, shutdown, segment, single_segment))
        // As many requests as the primary server's connections can carry at once, two each
        // (RD-108-25). Not the sum over every server: backups see only what the primary
        // refused, so anything beyond the primary's capacity would sit decoded in memory
        // waiting to be written. The per-file cap is already inside the pool.
        //
        // Unordered, because nothing here needs the order any more (RD-108-26): an article
        // is written where its own byte range says it belongs, so one that waits for a
        // second attempt no longer holds back the finished ones behind it - and, worse, no
        // longer stops the stream from asking for the next articles at all, which is what
        // left connections idle at every outlier and at the tail of every file.
        .buffer_unordered(pool.max_parallel_requests().max(1));
    tokio::pin!(fetches);
    let mut pending = PendingCheckpoints::default();
    loop {
        // Whatever is written is confirmed before the assembly waits for the network, so a
        // batch only grows while articles are already queueing up behind the writer.
        let next = match fetches.next().now_or_never() {
            Some(next) => next,
            None => {
                pending.flush(database, nzb_file.id, open).await?;
                fetches.next().await
            }
        };
        let Some(result) = next else {
            break;
        };
        open.answered();
        if let ControlFlow::Break(outcome) = take_in(
            result,
            &mut assembly,
            &mut pending,
            database,
            nzb_file,
            open,
            limits,
        )
        .await?
        {
            return Ok(outcome);
        }
    }
    // Every article is confirmed before the file's output path is, so a file whose path is
    // recorded never has a segment that is written but not `Completed`.
    pending.flush(database, nzb_file.id, open).await?;
    finish(
        assembly,
        database,
        shutdown,
        nzb_file,
        staging,
        destination,
        &part_path,
    )
    .await
}

/// One article asked of the pool, unless the shutdown comes first.
async fn fetch_segment(
    database: &Database,
    pool: &NntpPool,
    shutdown: &CancellationToken,
    segment: rd_core::NzbSegmentStatus,
    single_segment: bool,
) -> Result<FetchedSegment> {
    tokio::select! {
        () = shutdown.cancelled() => Ok(FetchedSegment::Cancelled),
        result = pool.fetch_decoded(&segment.message_id) => match result {
            Ok(article) => Ok(FetchedSegment::Article(
                Box::new(segment),
                article.article,
                article.attempts,
            )),
            Err(error) => {
                record_attempts(database, segment.id, pool.server_count()).await?;
                database.set_nzb_segment_state(segment.id, NzbSegmentState::Failed, None).await?;
                match error {
                    // Nobody answered the question, so nothing may be written in the
                    // article's place: a hole of zeros here is a file the user has to
                    // discover is broken (RD-108-29). The file goes back to the queue
                    // instead, with its `.part` file and its checkpoints intact.
                    FetchError::ServerFault(detail) => Err(server_unavailable_failure(&detail).into()),
                    FetchError::Unavailable(detail) if single_segment => {
                        Err(anyhow::anyhow!("{detail}"))
                    }
                    FetchError::Unavailable(detail) => {
                        Ok(FetchedSegment::Missing(Box::new(segment), detail))
                    }
                }
            }
        }
    }
}

/// Takes one answered article in: written into place and booked for the next checkpoint, or
/// counted as a hole. `Break` when the file ends here, cancelled; an error ends it failed.
/// Either way the articles written so far are confirmed and the ones in flight go back.
async fn take_in(
    result: Result<FetchedSegment>,
    assembly: &mut Assembly,
    pending: &mut PendingCheckpoints,
    database: &Database,
    nzb_file: &NzbFileStatus,
    open: &OpenArticles,
    limits: &rd_scheduler::RunLimits,
) -> Result<ControlFlow<FileOutcome>> {
    let (segment, decoded, attempts) = match result {
        Ok(FetchedSegment::Article(segment, decoded, attempts)) => (*segment, decoded, attempts),
        Ok(FetchedSegment::Missing(segment, detail)) => {
            // Like SABnzbd: keep going, leave a zero-filled hole and let PAR2 repair it.
            // At `debug`: an incomplete post is routine, and the file's one line with the
            // count, and a warning only if its set cannot be repaired, are what the log
            // carries (RD-1240-38, 543 per-segment warnings on the owner's instance).
            assembly.missing += 1;
            tracing::debug!(
                nzb_file_id = %nzb_file.id,
                segment = segment.number,
                message_id = %segment.message_id,
                detail,
                "segment missing on every server"
            );
            return Ok(ControlFlow::Continue(()));
        }
        Ok(FetchedSegment::Cancelled) => {
            pending
                .flush_before_leaving(database, nzb_file.id, open)
                .await;
            reset_inflight_segments(database, nzb_file).await?;
            return Ok(ControlFlow::Break(FileOutcome::Cancelled));
        }
        Err(error) => {
            pending
                .flush_before_leaving(database, nzb_file.id, open)
                .await;
            reset_inflight_segments(database, nzb_file).await?;
            return Err(error);
        }
    };
    let assembled = assembly.write(&decoded, nzb_file, limits).await;
    match assembled {
        Ok((part_begin, part_end)) => {
            pending.push(
                decoded.metadata.name.clone(),
                decoded.metadata.declared_size,
                rd_db::AssembledSegment {
                    segment_id: segment.id,
                    part_begin,
                    part_end,
                    crc32: decoded.crc32,
                    attempts,
                },
            );
            if pending.is_full() {
                pending.flush(database, nzb_file.id, open).await?;
            }
        }
        Err(error) => {
            record_attempts(database, segment.id, attempts as usize).await?;
            database
                .set_nzb_segment_state(segment.id, NzbSegmentState::Failed, None)
                .await?;
            // The articles before this one were written correctly; confirming them
            // spares the retry their download.
            pending
                .flush_before_leaving(database, nzb_file.id, open)
                .await;
            reset_inflight_segments(database, nzb_file).await?;
            return Err(error);
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Completes the assembled file: checks its size, fills the holes of missing articles,
/// settles its name and moves it from staging to its place in `destination`.
async fn finish(
    assembly: Assembly,
    database: &Database,
    shutdown: &CancellationToken,
    nzb_file: &NzbFileStatus,
    staging: &std::path::Path,
    destination: &std::path::Path,
    part_path: &std::path::Path,
) -> Result<FileOutcome> {
    let Assembly {
        mut output,
        name,
        declared_size,
        written,
        deviations,
        missing,
        single_segment: _,
    } = assembly;
    let expected = match declared_size {
        Some(size) => size,
        None if missing > 0 => {
            return Err(missing_everything_failure(&nzb_file.subject).into());
        }
        None => bail!("missing yEnc size"),
    };
    // Again here, for a size a resumed file brought along rather than an article (RD-1101-16).
    crate::bounds::check_declared_size(expected, nzb_file)?;
    // The file is as long as the articles said it is, whatever order they arrived in, and
    // every byte no article covered is a hole where the missing article belongs.
    let holes = written.holes(expected);
    if missing == 0 && !holes.is_empty() {
        let bytes: u64 = holes.iter().map(|(begin, end)| end - begin + 1).sum();
        bail!("assembled yEnc size mismatch: {bytes} bytes of {expected} were never written");
    }
    let gaps = crate::bounds::Gaps {
        database,
        staging,
        shutdown,
    };
    if !gaps.fill(&mut output, expected, &holes).await? {
        return Ok(FileOutcome::Cancelled);
    }
    output.sync_all().await?;
    drop(output);
    // Obfuscating posters vary the yEnc name per article or drop the extension; the NZB
    // subject then carries the real name (SABnzbd behaves the same way).
    let yenc_name = name.as_deref().context("missing yEnc filename")?;
    let subject_name = rd_collector::subject_file_name(&nzb_file.subject);
    let chosen = if deviations.any() || !rd_collector::looks_like_file_name(yenc_name) {
        subject_name.as_deref().unwrap_or(yenc_name)
    } else {
        yenc_name
    };
    let final_name = sanitize_file_name(chosen);
    // One line for the file, after the name is settled -- not one per article.
    deviations.report(&final_name);
    let final_path = collision_free_path(destination, &final_name);
    // Persist the intended path first: after a crash the worker can either resume the
    // still-present `.part` file or recognize the already-renamed final file.
    database
        .checkpoint_nzb_file_output(
            nzb_file.id,
            final_path
                .to_str()
                .context("NZB output path is not UTF-8")?
                .to_owned(),
        )
        .await?;
    tokio::fs::rename(part_path, &final_path)
        .await
        .with_context(|| format!("commit decoded file {}", final_path.display()))?;
    Ok(FileOutcome::Completed {
        path: final_path,
        missing,
    })
}

enum FetchedSegment {
    /// The article, with the number of servers it took.
    Article(Box<rd_core::NzbSegmentStatus>, crate::DecodedArticle, u32),
    Missing(Box<rd_core::NzbSegmentStatus>, String),
    Cancelled,
}

pub(crate) enum FileOutcome {
    Completed { path: PathBuf, missing: usize },
    Cancelled,
}

async fn reset_inflight_segments(database: &Database, file: &NzbFileStatus) -> Result<()> {
    let files = database.list_nzb_files(file.import_id).await?;
    let Some(current) = files.into_iter().find(|candidate| candidate.id == file.id) else {
        bail!("NZB file disappeared while resetting segment states");
    };
    for segment in current.segments {
        if segment.state == NzbSegmentState::Downloading {
            database
                .set_nzb_segment_state(segment.id, NzbSegmentState::Queued, None)
                .await?;
        }
    }
    Ok(())
}

/// Books `attempts` server attempts on a segment that has no checkpoint to carry them.
async fn record_attempts(
    database: &Database,
    segment_id: rd_core::NzbSegmentId,
    attempts: usize,
) -> Result<()> {
    for _ in 0..attempts {
        database
            .set_nzb_segment_state(segment_id, NzbSegmentState::Downloading, None)
            .await?;
    }
    Ok(())
}
