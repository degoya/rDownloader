//! Joining a livestream recording's segments into one container (RD-080-09).
//!
//! A persistent post-processing step rather than something the recorder does before it
//! returns, for two reasons. A remux of a six-hour recording takes minutes and must survive a
//! restart in the middle — the checkpoint is what makes that a resumed step instead of a lost
//! one. And it reports progress through the same machinery every other step does, so a
//! recording being converted looks like every other package being worked on.
//!
//! The segments are concatenated, not re-encoded: `-c copy` is the whole point. Re-encoding a
//! recording would take hours and lose quality to no purpose.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState, PostprocessStep};
use rd_files::NoConsoleWindow as _;

use crate::{
    Inner,
    steps::{checkpoint, find_step, path_string},
};

/// How long one remux may take before it is abandoned.
const REMUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// What one remux needs.
pub(crate) struct RemuxContext<'a> {
    pub owner: &'a str,
    pub directory: &'a Path,
    /// Segment files in recording order.
    pub segments: Vec<PathBuf>,
    /// Container to produce.
    pub target: rd_core::RemuxTarget,
    /// Name of the finished file, without its extension.
    pub stem: String,
    pub ffmpeg: PathBuf,
}

/// Joins the segments; `false` when the remux failed and the segments were kept.
pub(crate) async fn run(
    inner: &Inner,
    steps: &[PostprocessStep],
    context: &RemuxContext<'_>,
) -> Result<bool> {
    let Some(extension) = context.target.extension() else {
        return Ok(true);
    };
    if context.segments.is_empty() {
        return Ok(true);
    }
    let output = context
        .directory
        .join(format!("{}.{extension}", context.stem));
    let source = path_string(&output)?;
    if find_step(steps, PostprocessKind::Remux, &source)
        .is_some_and(|step| step.state == PostprocessState::Completed)
    {
        return Ok(true);
    }

    crate::steps::stage(
        inner,
        context.owner,
        PostprocessStage::Remuxing,
        output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    )
    .await?;
    checkpoint(
        inner,
        context.owner,
        PostprocessKind::Remux,
        &source,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;

    // The concat *demuxer* rather than the `concat:` protocol: it is the one that works for
    // MPEG-TS segments of differing stream parameters, which is exactly what a reconnect
    // produces when the provider changes bitrate mid-stream.
    let list = context
        .directory
        .join(format!("{}.concat.txt", context.stem));
    let manifest = context
        .segments
        .iter()
        .map(|segment| {
            let name = segment
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Single quotes are escaped the way the demuxer expects; a stream title is not
            // something we control.
            format!("file '{}'\n", name.replace('\'', "'\\''"))
        })
        .collect::<String>();
    tokio::fs::write(&list, manifest).await?;

    let result = run_ffmpeg(&context.ffmpeg, context.directory, &list, &output).await;
    let _ = tokio::fs::remove_file(&list).await;

    match result {
        Ok(()) => {
            checkpoint(
                inner,
                context.owner,
                PostprocessKind::Remux,
                &source,
                PostprocessState::Completed,
                None,
                None,
            )
            .await?;
            // The segments are removed only once the container exists and is non-empty:
            // deleting a recording because a remux half-worked is unrecoverable.
            if tokio::fs::metadata(&output)
                .await
                .is_ok_and(|meta| meta.len() > 0)
            {
                for segment in &context.segments {
                    let _ = tokio::fs::remove_file(segment).await;
                }
            }
            Ok(true)
        }
        Err(error) => {
            let message = rd_core::redact_text(&error.to_string());
            tracing::warn!(%message, "remux failed; the recording's segments were kept");
            crate::steps::checkpoint_coded(
                inner,
                context.owner,
                PostprocessKind::Remux,
                &source,
                PostprocessState::Failed,
                None,
                crate::steps::Outcome::detailed(crate::steps::codes::REMUX_FAILED, message),
            )
            .await?;
            // A failed remux leaves a partial container behind that nothing should mistake
            // for the recording.
            let _ = tokio::fs::remove_file(&output).await;
            Ok(false)
        }
    }
}

/// The ffmpeg arguments that join the segments `list` names into `output`.
///
/// `-safe 0` because a segment is named after a stream title, which the demuxer's safe mode
/// refuses for its characters alone; every name in the list is a bare file name in the
/// recording's own folder (see `run`). What a segment holds is the provider's, though, and the
/// concat demuxer probes each one: a playlist served as a segment would open its own inputs.
/// `-protocol_whitelist file` keeps every input local - the concat demuxer and the demuxers
/// below it hand the list on to what they open (RD-1101-16, audit S18).
fn ffmpeg_arguments(list: &Path, output: &Path) -> Vec<std::ffi::OsString> {
    let mut arguments: Vec<std::ffi::OsString> = [
        "-nostdin",
        "-y",
        "-protocol_whitelist",
        "file",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .collect();
    arguments.push(list.as_os_str().to_owned());
    // Stream copy: a recording must not be re-encoded, which would take hours and lose
    // quality for nothing.
    arguments.extend(["-c", "copy"].map(std::ffi::OsString::from));
    arguments.push(output.as_os_str().to_owned());
    arguments
}

async fn run_ffmpeg(ffmpeg: &Path, directory: &Path, list: &Path, output: &Path) -> Result<()> {
    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .current_dir(directory)
        .args(ffmpeg_arguments(list, output))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .no_console_window();
    // Nothing of the service's environment but the allowlist (security review 2026-09-28,
    // finding 7).
    rd_postprocess::restrict_environment(&mut command, &[]);

    let output_result = tokio::time::timeout(REMUX_TIMEOUT, command.output())
        .await
        .map_err(|_| anyhow::anyhow!("remux timed out"))??;
    if output_result.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output_result.stderr);
    anyhow::bail!("{}", rd_tools::stderr_tail(&stderr, "ffmpeg failed"))
}

/// The segment files of a recording, in recording order.
///
/// Order comes from the stored history rather than from the directory listing: the history is
/// what knows which attempt produced which file, and a directory read would depend on the
/// names sorting correctly — which they do, but relying on it twice is one place too many.
#[must_use]
pub(crate) fn segments_of(state: &rd_core::RecordingState, directory: &Path) -> Vec<PathBuf> {
    let mut segments: Vec<&rd_core::RecordingSegment> = state.segments.iter().collect();
    segments.sort_by_key(|segment| segment.index);
    segments
        .into_iter()
        .map(|segment| directory.join(&segment.file_name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ffmpeg_arguments, segments_of};
    use chrono::Utc;
    use rd_core::{RecordingSegment, RecordingState, SegmentEnd};

    fn segment(index: u32, name: &str) -> RecordingSegment {
        RecordingSegment {
            index,
            file_name: name.to_owned(),
            bytes: 10,
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            reason: SegmentEnd::Finished,
        }
    }

    #[test]
    fn segments_are_ordered_by_their_index_not_by_insertion() {
        // The remux joins them in this order, so a history that arrived out of order — a
        // resumed recording appending to an older row — must still produce the right film.
        let state = RecordingState {
            segments: vec![
                segment(3, "show.part003.ts"),
                segment(1, "show.part001.ts"),
                segment(2, "show.part002.ts"),
            ],
            ..RecordingState::default()
        };
        let paths = segments_of(&state, std::path::Path::new("/tmp/pkg"));
        let names: Vec<String> = paths
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("segment paths always end in a file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec!["show.part001.ts", "show.part002.ts", "show.part003.ts"]
        );
    }

    #[test]
    fn a_recording_with_no_segments_yields_nothing_to_join() {
        assert!(segments_of(&RecordingState::default(), std::path::Path::new("/tmp")).is_empty());
    }

    /// RD-1101-16 (audit S18): every input the join opens is a local file, and the whitelist
    /// is an input option, so it stands before the `-i` it applies to.
    #[test]
    fn the_join_opens_local_files_only() {
        let arguments: Vec<String> = ffmpeg_arguments(
            std::path::Path::new("show.concat.txt"),
            std::path::Path::new("show.mkv"),
        )
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
        let whitelist = arguments
            .iter()
            .position(|argument| argument == "-protocol_whitelist")
            .expect("a protocol whitelist");
        assert_eq!(arguments[whitelist + 1], "file");
        let input = arguments
            .iter()
            .position(|argument| argument == "-i")
            .expect("an input");
        assert!(whitelist < input);
        assert_eq!(arguments[input + 1], "show.concat.txt");
        assert_eq!(arguments.last().map(String::as_str), Some("show.mkv"));
    }
}
