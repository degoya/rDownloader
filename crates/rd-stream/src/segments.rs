//! Recording one stream as a sequence of segments (RD-080-09).
//!
//! A long recording is not one process writing one file. It is a series of attempts: the
//! split policy ends a segment deliberately, a dropped connection ends one accidentally, and
//! both continue into the next. The rule that shapes everything here is that **bytes already
//! on disk are never given up** — a disconnect three hours in must cost the three hours it
//! has not recorded yet, and nothing else.
//!
//! The decisions are pure functions over the state so far; only [`record`] touches a process.

use std::{path::Path, time::Duration};

use anyhow::Result;
use chrono::Utc;
use rd_core::{
    MAX_RECONNECTS, MAX_SEGMENTS, RecordingPolicy, RecordingSegment, RecordingState, SegmentEnd,
    segment_name,
};
use rd_tools::{ToolProcess, process::Stdout};
use tokio_util::sync::CancellationToken;

/// How often the running segment's size is sampled.
const SIZE_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Extension streamlink writes; MPEG-TS survives a hard kill, which is why segments are
/// concatenable and why a killed recording is still playable.
pub const SEGMENT_EXTENSION: &str = "ts";

/// What one segment's process did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentOutcome {
    /// The stream ended by itself.
    StreamEnded,
    /// The split policy cut it; the next segment follows immediately.
    Split,
    /// The process died unexpectedly with bytes on disk.
    Dropped,
    /// The user stopped the recording.
    Cancelled,
    /// The process ended without writing anything.
    Empty,
}

/// Whether another segment should be attempted after `outcome`.
///
/// A deliberate split always continues. A drop continues only while there is reconnect budget
/// left, because a stream that is genuinely over looks exactly like one that dropped, and
/// retrying forever would keep a finished recording open indefinitely.
#[must_use]
pub fn should_continue(outcome: SegmentOutcome, reconnects: u32, segments: u32) -> bool {
    if segments >= MAX_SEGMENTS {
        return false;
    }
    match outcome {
        SegmentOutcome::Split => true,
        SegmentOutcome::Dropped => reconnects < MAX_RECONNECTS,
        // Nothing was written, so there is nothing to protect and no reason to think the
        // next attempt would differ.
        SegmentOutcome::StreamEnded | SegmentOutcome::Cancelled | SegmentOutcome::Empty => false,
    }
}

/// How a segment's outcome is recorded on the segment it ended.
#[must_use]
pub const fn end_reason(outcome: SegmentOutcome) -> SegmentEnd {
    match outcome {
        SegmentOutcome::Split => SegmentEnd::Split,
        SegmentOutcome::Dropped => SegmentEnd::Disconnect,
        SegmentOutcome::StreamEnded | SegmentOutcome::Cancelled | SegmentOutcome::Empty => {
            SegmentEnd::Finished
        }
    }
}

/// What to record and with what.
pub struct SegmentTool<'a> {
    pub streamlink: &'a Path,
    pub url: &'a str,
    pub quality: &'a str,
}

/// Records one segment, returning what ended it and how many bytes it holds.
///
/// `on_progress` is called with the running total across the whole recording, so the queue
/// row shows a number that only ever grows — a segment boundary must not make the progress
/// jump backwards.
pub async fn record<F>(
    tool: &SegmentTool<'_>,
    output: &Path,
    policy: RecordingPolicy,
    carried_bytes: u64,
    cancellation: &CancellationToken,
    mut on_progress: F,
) -> Result<(SegmentOutcome, u64, String)>
where
    F: FnMut(u64),
{
    let SegmentTool {
        streamlink,
        url,
        quality,
    } = tool;
    let mut command = tokio::process::Command::new(streamlink);
    command
        .arg("--output")
        .arg(output)
        // A retry after a failed attempt reuses the same segment name.
        .arg("--force")
        .arg("--")
        .arg(url)
        .arg(quality);
    // Discarded, not captured: streamlink writes the stream itself to `--output` and nothing
    // here parses its stdout. Piping it would only fill a buffer nobody drains, which stalls
    // a recording that can run for hours.
    let mut process = ToolProcess::spawn(&mut command, "streamlink", Stdout::Discarded)?;

    let started = std::time::Instant::now();
    let mut ticker = tokio::time::interval(SIZE_POLL_INTERVAL);
    let mut bytes: u64 = 0;
    let mut split = false;

    let status = loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                // MPEG-TS needs no finalisation, so a hard kill leaves a playable file and
                // everything recorded so far is kept.
                process.kill().await;
                break None;
            }
            status = process.wait() => break Some(status?),
            _ = ticker.tick() => {
                if let Ok(meta) = tokio::fs::metadata(output).await {
                    bytes = meta.len();
                    on_progress(carried_bytes + bytes);
                }
                if policy.split.should_split(started.elapsed().as_secs(), bytes) {
                    // Ending the process is what cuts the file. The next segment opens
                    // immediately, so nothing is lost at the boundary.
                    process.kill().await;
                    split = true;
                    break None;
                }
            }
        }
    };

    if let Ok(meta) = tokio::fs::metadata(output).await {
        bytes = meta.len();
    }
    let stderr_text = process.stderr().await;

    let outcome = if split {
        SegmentOutcome::Split
    } else if bytes == 0 {
        SegmentOutcome::Empty
    } else {
        match status {
            // Killed by the cancellation branch.
            None => SegmentOutcome::Cancelled,
            Some(status) if status.success() => SegmentOutcome::StreamEnded,
            Some(_) => SegmentOutcome::Dropped,
        }
    };
    Ok((outcome, bytes, stderr_text))
}

/// Appends a finished segment to the recording's history.
pub fn push_segment(
    state: &mut RecordingState,
    index: u32,
    file_name: String,
    bytes: u64,
    started_at: chrono::DateTime<Utc>,
    outcome: SegmentOutcome,
) {
    state.segments.push(RecordingSegment {
        index,
        file_name,
        bytes,
        started_at,
        ended_at: Some(Utc::now()),
        reason: end_reason(outcome),
    });
    if outcome == SegmentOutcome::Dropped {
        state.reconnects = state.reconnects.saturating_add(1);
    }
}

/// The name of the next segment file.
#[must_use]
pub fn next_name(stem: &str, index: u32) -> String {
    segment_name(stem, index, SEGMENT_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::{SegmentOutcome, end_reason, next_name, push_segment, should_continue};
    use chrono::Utc;
    use rd_core::{MAX_RECONNECTS, MAX_SEGMENTS, RecordingState, SegmentEnd};

    #[test]
    fn a_drop_reconnects_while_there_is_budget() {
        assert!(should_continue(SegmentOutcome::Dropped, 0, 1));
        assert!(should_continue(
            SegmentOutcome::Dropped,
            MAX_RECONNECTS - 1,
            1
        ));
        // Bounded: a stream that has genuinely ended looks exactly like one that dropped,
        // and retrying forever would keep a finished recording open.
        assert!(!should_continue(SegmentOutcome::Dropped, MAX_RECONNECTS, 1));
    }

    #[test]
    fn a_split_always_continues_and_an_ending_never_does() {
        assert!(should_continue(SegmentOutcome::Split, 0, 1));
        assert!(!should_continue(SegmentOutcome::StreamEnded, 0, 1));
        assert!(!should_continue(SegmentOutcome::Cancelled, 0, 1));
        // Nothing was written, so there is nothing to protect.
        assert!(!should_continue(SegmentOutcome::Empty, 0, 1));
    }

    #[test]
    fn the_segment_count_is_bounded() {
        assert!(!should_continue(SegmentOutcome::Split, 0, MAX_SEGMENTS));
    }

    #[test]
    fn only_a_disconnect_records_a_gap() {
        assert_eq!(end_reason(SegmentOutcome::Split), SegmentEnd::Split);
        assert_eq!(end_reason(SegmentOutcome::Dropped), SegmentEnd::Disconnect);
        assert_eq!(
            end_reason(SegmentOutcome::StreamEnded),
            SegmentEnd::Finished
        );
        assert!(end_reason(SegmentOutcome::Dropped).leaves_gap());
        assert!(!end_reason(SegmentOutcome::Split).leaves_gap());
    }

    #[test]
    fn a_reconnect_is_counted_and_a_split_is_not() {
        let mut state = RecordingState::default();
        push_segment(
            &mut state,
            1,
            next_name("show", 1),
            100,
            Utc::now(),
            SegmentOutcome::Split,
        );
        assert_eq!(state.reconnects, 0);
        push_segment(
            &mut state,
            2,
            next_name("show", 2),
            200,
            Utc::now(),
            SegmentOutcome::Dropped,
        );
        assert_eq!(state.reconnects, 1);
        // And what was already recorded is still there — the point of the whole loop.
        assert_eq!(state.total_bytes(), 300);
        assert!(state.has_gaps());
    }

    #[test]
    fn segment_names_carry_their_index() {
        assert_eq!(next_name("show", 1), "show.part001.ts");
        assert_eq!(next_name("show", 42), "show.part042.ts");
    }
}
