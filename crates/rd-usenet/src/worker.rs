//! NZB file assembly: segment download, resume, gap filling and final placement.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use rd_core::{Failure, FailureKind, NzbFileStatus, NzbSegmentState};
use rd_db::Database;
use rd_files::{collision_free_path, sanitize_file_name};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::{NntpPool, pool::FetchError, segments::NameDeviations};

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

pub(crate) async fn download_file(
    database: &Database,
    pool: &NntpPool,
    shutdown: &CancellationToken,
    nzb_file: &NzbFileStatus,
    staging: &std::path::Path,
    destination: &std::path::Path,
    limits: &rd_scheduler::RunLimits,
) -> Result<FileOutcome> {
    let part_path = staging.join(format!("{}.part", nzb_file.id));
    if nzb_file.segments.is_empty() {
        bail!("NZB file contains no segments");
    }
    let resume = crate::assembly_resume::prepare(database, nzb_file, &part_path).await?;
    let mut output = resume.output;
    let mut name = resume.name;
    let mut declared_size = resume.declared_size;
    let mut written = Covered::resumed(resume.written)?;
    let segments = resume.remaining;
    let single_segment = nzb_file.segments.len() == 1;
    let fetches = futures_util::stream::iter(segments).map(|segment| async move {
        database
            .set_nzb_segment_state(segment.id, NzbSegmentState::Downloading, None)
            .await?;
        tokio::select! {
            () = shutdown.cancelled() => {
                database.set_nzb_segment_state(segment.id, NzbSegmentState::Queued, None).await?;
                Ok(FetchedSegment::Cancelled)
            }
            result = pool.fetch_decoded(&segment.message_id) => match result {
                Ok(article) => {
                    record_additional_attempts(database, segment.id, article.attempts as usize).await?;
                    Ok(FetchedSegment::Article(Box::new(segment), article.article))
                }
                Err(error) => {
                    record_additional_attempts(database, segment.id, pool.server_count()).await?;
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
    })
    // As many requests as the primary server's connections can carry at once, two each
    // (RD-108-25). Not the sum over every server: backups see only what the primary refused,
    // so anything beyond the primary's capacity would sit decoded in memory waiting to be
    // written. The per-file cap is already inside the pool.
    //
    // Unordered, because nothing here needs the order any more (RD-108-26): an article is
    // written where its own byte range says it belongs, so one that waits for a second
    // attempt no longer holds back the finished ones behind it - and, worse, no longer stops
    // the stream from asking for the next articles at all, which is what left connections
    // idle at every outlier and at the tail of every file.
    .buffer_unordered(pool.max_parallel_requests().max(1));
    tokio::pin!(fetches);
    let mut missing = 0_usize;
    let mut deviations = NameDeviations::default();
    while let Some(result) = fetches.next().await {
        let (segment, decoded) = match result {
            Ok(FetchedSegment::Article(segment, decoded)) => (*segment, decoded),
            Ok(FetchedSegment::Missing(segment, detail)) => {
                // Like SABnzbd: keep going, leave a zero-filled hole and let PAR2 repair it.
                missing += 1;
                tracing::warn!(
                    nzb_file_id = %nzb_file.id,
                    segment = segment.number,
                    message_id = %segment.message_id,
                    detail,
                    "segment missing on every server"
                );
                continue;
            }
            Ok(FetchedSegment::Cancelled) => {
                reset_inflight_segments(database, nzb_file).await?;
                return Ok(FileOutcome::Cancelled);
            }
            Err(error) => {
                reset_inflight_segments(database, nzb_file).await?;
                return Err(error);
            }
        };
        let checkpoint = async {
            // Paces the assembly, which back-pressures the bounded fetch stream behind it —
            // so the NNTP transport honours the same limits as every other transport.
            limits.bandwidth.acquire(decoded.data.len()).await?;
            deviations.observe(name.as_deref(), &decoded.metadata.name);
            validate_metadata(&decoded, &mut name, &mut declared_size)?;
            let (part_begin, part_end) = decoded_part_range(&decoded, single_segment)?;
            written.claim(part_begin, part_end, &decoded, declared_size)?;
            output
                .seek(std::io::SeekFrom::Start(part_begin - 1))
                .await?;
            output.write_all(&decoded.data).await?;
            // No `sync_data()` here, on purpose (RD-108-25). It looked like a durability
            // guarantee and was a throughput ceiling: one fdatasync per article, in this
            // serial loop, 3.9 ms each on the NVMe it was measured on - a hard cap near
            // 200 MB/s, far lower on NTFS or a spinning disk. The resume never needed it.
            // `assembly_resume::prepare` trusts no checkpoint: on restart it CRC-checks every
            // range the database calls complete against the bytes actually on disk, stops at
            // the first short file or mismatch, truncates to the last proven byte and queues
            // the rest again. Bytes the kernel had not flushed when the power went cost the
            // segments they belonged to, never the file. The one sync that matters, before
            // the rename, stays below. SABnzbd's assembler forces no write either.
            rd_core::failpoint!("usenet.after_article_write", || {
                anyhow::anyhow!("crash point: usenet.after_article_write")
            });
            database
                .checkpoint_nzb_assembly_segment(
                    nzb_file.id,
                    segment.id,
                    decoded.metadata.name.clone(),
                    decoded.metadata.declared_size,
                    part_begin,
                    part_end,
                    decoded.crc32,
                )
                .await?;
            Result::<()>::Ok(())
        }
        .await;
        if let Err(error) = checkpoint {
            database
                .set_nzb_segment_state(segment.id, NzbSegmentState::Failed, None)
                .await?;
            reset_inflight_segments(database, nzb_file).await?;
            return Err(error);
        }
    }
    let expected = match declared_size {
        Some(size) => size,
        None if missing > 0 => {
            return Err(missing_everything_failure(&nzb_file.subject).into());
        }
        None => bail!("missing yEnc size"),
    };
    // The file is as long as the articles said it is, whatever order they arrived in, and
    // every byte no article covered is a hole where the missing article belongs. Written
    // rather than left to the file system: a `.part` file resumed after a crash can hold the
    // remains of an article that was interrupted mid-write, and those bytes are not zeros.
    output.set_len(expected).await?;
    let holes = written.holes(expected);
    if missing == 0 && !holes.is_empty() {
        let bytes: u64 = holes.iter().map(|(begin, end)| end - begin + 1).sum();
        bail!("assembled yEnc size mismatch: {bytes} bytes of {expected} were never written");
    }
    for (begin, end) in holes {
        write_zeros(&mut output, begin, end).await?;
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
    tokio::fs::rename(&part_path, &final_path)
        .await
        .with_context(|| format!("commit decoded file {}", final_path.display()))?;
    Ok(FileOutcome::Completed {
        path: final_path,
        missing,
    })
}

/// The byte ranges articles have filled, 1-based and inclusive.
///
/// What `written: u64` used to say when assembly was strictly front to back (RD-108-26).
/// It answers the two questions that took its place: may this article be written here, and
/// what is left over at the end.
#[derive(Default)]
struct Covered(std::collections::BTreeMap<u64, u64>);

impl Covered {
    /// The ranges a resumed `.part` file already holds, proven by checksum.
    fn resumed(ranges: Vec<(u64, u64)>) -> Result<Self> {
        let mut covered = Self::default();
        for (begin, end) in ranges {
            covered.insert(begin, end)?;
        }
        Ok(covered)
    }

    /// Books an article's range, if the range is one this file can hold and nothing has
    /// written there yet.
    fn claim(
        &mut self,
        begin: u64,
        end: u64,
        decoded: &crate::DecodedArticle,
        declared_size: Option<u64>,
    ) -> Result<()> {
        if begin == 0 || end < begin {
            bail!("invalid yEnc part range {begin}-{end}");
        }
        if end - begin + 1 != decoded.data.len() as u64 {
            bail!("yEnc part range does not match decoded length");
        }
        if declared_size.is_some_and(|size| end > size) {
            bail!("yEnc part range {begin}-{end} lies outside the announced file size");
        }
        self.insert(begin, end)
    }

    fn insert(&mut self, begin: u64, end: u64) -> Result<()> {
        // Two articles claiming the same bytes means one of them is not the article it says
        // it is; writing both would leave whichever landed last, silently.
        if let Some((_, earlier_end)) = self.0.range(..=begin).next_back()
            && *earlier_end >= begin
        {
            bail!("overlapping yEnc part range {begin}-{end}");
        }
        if let Some((later_begin, _)) = self.0.range(begin..).next()
            && *later_begin <= end
        {
            bail!("overlapping yEnc part range {begin}-{end}");
        }
        self.0.insert(begin, end);
        Ok(())
    }

    /// The ranges of a file of `size` bytes that no article covered.
    fn holes(&self, size: u64) -> Vec<(u64, u64)> {
        let mut holes = Vec::new();
        let mut next = 1_u64;
        for (begin, end) in &self.0 {
            if *begin > next {
                holes.push((next, begin - 1));
            }
            next = end.saturating_add(1);
        }
        if next <= size {
            holes.push((next, size));
        }
        holes
    }
}

/// Writes zeros over the 1-based inclusive range `begin..=end`.
async fn write_zeros(output: &mut tokio::fs::File, begin: u64, end: u64) -> Result<()> {
    static ZEROS: [u8; 64 * 1024] = [0; 64 * 1024];
    output.seek(std::io::SeekFrom::Start(begin - 1)).await?;
    let mut remaining = end - begin + 1;
    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(ZEROS.len() as u64))?;
        output.write_all(&ZEROS[..chunk]).await?;
        remaining -= chunk as u64;
    }
    Ok(())
}

/// Where this article belongs in the file.
///
/// A post without `=ypart` names no place, so it can only be a file of one article - which
/// is what the old contiguity check said in its own way.
fn decoded_part_range(decoded: &crate::DecodedArticle, single_segment: bool) -> Result<(u64, u64)> {
    match (decoded.metadata.part_begin, decoded.metadata.part_end) {
        (Some(begin), Some(end)) => Ok((begin, end)),
        (None, None) if single_segment => Ok((1, u64::try_from(decoded.data.len())?)),
        (None, None) => bail!("multiple yEnc segments require explicit part ranges"),
        _ => bail!("incomplete yEnc part range"),
    }
}

enum FetchedSegment {
    Article(Box<rd_core::NzbSegmentStatus>, crate::DecodedArticle),
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

async fn record_additional_attempts(
    database: &Database,
    segment_id: rd_core::NzbSegmentId,
    attempts: usize,
) -> Result<()> {
    for _ in 1..attempts {
        database
            .set_nzb_segment_state(segment_id, NzbSegmentState::Downloading, None)
            .await?;
    }
    Ok(())
}

fn validate_metadata(
    decoded: &crate::DecodedArticle,
    name: &mut Option<String>,
    declared_size: &mut Option<u64>,
) -> Result<()> {
    if declared_size.is_some_and(|value| value != decoded.metadata.declared_size) {
        bail!("yEnc segments disagree on the declared file size");
    }
    name.get_or_insert_with(|| decoded.metadata.name.clone());
    declared_size.get_or_insert(decoded.metadata.declared_size);
    Ok(())
}
