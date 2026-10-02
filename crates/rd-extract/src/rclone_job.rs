//! rclone upload of a finished package folder to a configured remote
//! (`remote:path/<package>`), with progress parsed from rclone's JSON log.
//!
//! The upload limit (RD-150-15) reaches rclone as `--bwlimit`, the rate in force when the run
//! starts. A profile switch during a run applies from the next run on: rclone can change its
//! rate live only through its remote-control server — a listening port per upload — and
//! restarting it mid-file throws away the partial file, which costs more than a stale rate.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState};
use rd_files::NoConsoleWindow as _;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    Inner,
    steps::{checkpoint, truncate},
};

pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_millis(750);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UploadMode {
    Copy,
    Move,
}

impl UploadMode {
    /// The configured mode string; anything unknown falls back to the safe `copy`.
    pub(crate) fn from_setting(value: &str) -> Self {
        if value.eq_ignore_ascii_case("move") {
            Self::Move
        } else {
            Self::Copy
        }
    }

    const fn verb(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }
}

pub(crate) struct UploadContext<'a> {
    pub remote: &'a str,
    pub mode: UploadMode,
    pub package_name: &'a str,
    pub directory: &'a Path,
    pub executable: Option<&'a str>,
    pub vendor_directory: Option<&'a str>,
    /// The upload limit in bytes per second when the run starts; `None` = unlimited.
    pub bwlimit: Option<u64>,
}

/// `remote:path/<sanitized package name>` — the package keeps its own folder remotely.
pub(crate) fn destination(remote: &str, package_name: &str) -> String {
    format!(
        "{}/{}",
        remote.trim_end_matches('/'),
        rd_files::sanitize_file_name(package_name)
    )
}

/// The `--bwlimit` value for a rate in bytes per second; rclone reads a bare number as KiB/s,
/// so the `B` suffix is what keeps the rate exact.
pub(crate) fn bwlimit(bytes_per_second: Option<u64>) -> Option<String> {
    bytes_per_second
        .filter(|rate| *rate > 0)
        .map(|rate| format!("{rate}B"))
}

/// The rclone invocation for one upload.
fn command(tool: &Path, context: &UploadContext<'_>, target: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(tool);
    command.no_console_window();
    // The allowlist plus rclone's own configuration, config password and proxy variables
    // (security review 2026-09-28, finding 7): nothing else of the service's environment.
    rd_postprocess::restrict_environment(&mut command, rd_postprocess::RCLONE_VARIABLES);
    command
        .arg(context.mode.verb())
        .args(["--use-json-log", "--stats", "1s", "-v"]);
    if let Some(rate) = bwlimit(context.bwlimit) {
        command.args(["--bwlimit", &rate]);
    }
    if context.mode == UploadMode::Move {
        command.arg("--delete-empty-src-dirs");
    }
    command
        // `--` before the positionals, as every other external invocation in the tree does:
        // a configured remote or a package directory beginning with `-` is otherwise read by
        // rclone as a flag rather than as a path. Every flag comes first, since a flag after
        // `--` is read as a third path.
        .arg("--")
        .arg(context.directory)
        .arg(target);
    command
}

/// Transferred/total bytes from one rclone `--use-json-log` stats line.
pub(crate) fn parse_stats_line(line: &str) -> Option<(u64, Option<u64>)> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let stats = value.get("stats")?;
    let bytes = stats.get("bytes")?.as_u64()?;
    let total = stats.get("totalBytes").and_then(|value| value.as_u64());
    Some((bytes, total))
}

pub(crate) fn percent(bytes: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    Some(u8::try_from((bytes.min(total) * 100) / total).unwrap_or(100))
}

/// Runs the upload step; `Ok(true)` when rclone exited with status 0.
pub(crate) async fn run(inner: &Inner, owner: &str, context: &UploadContext<'_>) -> Result<bool> {
    let target = destination(context.remote, context.package_name);
    crate::steps::stage(
        inner,
        owner,
        PostprocessStage::Uploading,
        Some(target.clone()),
    )
    .await?;
    checkpoint(
        inner,
        owner,
        PostprocessKind::Upload,
        context.remote,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    let outcome = execute(inner, owner, context, &target).await;
    let (state, ok, output, message) = match outcome {
        Ok((true, _)) => (PostprocessState::Completed, true, Some(target), None),
        Ok((false, tail)) => (PostprocessState::Failed, false, None, Some(tail)),
        Err(error) => (
            PostprocessState::Failed,
            false,
            None,
            Some(error.to_string()),
        ),
    };
    checkpoint(
        inner,
        owner,
        PostprocessKind::Upload,
        context.remote,
        state,
        output,
        message.map(truncate),
    )
    .await?;
    Ok(ok)
}

async fn execute(
    inner: &Inner,
    owner: &str,
    context: &UploadContext<'_>,
    target: &str,
) -> Result<(bool, String)> {
    let Some(tool) = rd_core::locate_tool(context.executable, context.vendor_directory, "rclone")
    else {
        anyhow::bail!("rclone not found (settings, vendor folder or PATH)");
    };
    let mut command = command(&tool.path, context, target);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("spawn rclone")?;
    let stderr = child.stderr.take().context("rclone stderr")?;
    let mut lines = BufReader::new(stderr).lines();
    let mut tail: Vec<String> = Vec::new();
    let mut last_progress = std::time::Instant::now() - PROGRESS_INTERVAL;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some((bytes, total)) = parse_stats_line(&line) {
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = std::time::Instant::now();
                let _ = inner
                    .database
                    .postprocess_progress(
                        owner.to_owned(),
                        PostprocessKind::Upload,
                        context.remote.to_owned(),
                        PostprocessStage::Uploading,
                        percent(bytes, total),
                        Some(target.to_owned()),
                    )
                    .await;
            }
            continue;
        }
        // Non-stats log lines: keep a short tail as the failure message.
        if tail.len() >= 20 {
            tail.remove(0);
        }
        tail.push(line);
    }
    let status = child.wait().await.context("wait for rclone")?;
    let summary = if status.success() {
        String::new()
    } else {
        format!(
            "rclone exit status {}\n{}",
            status.code().unwrap_or(-1),
            tail.join("\n")
        )
    };
    Ok((status.success(), summary))
}

#[cfg(test)]
mod tests {
    use super::{
        UploadContext, UploadMode, bwlimit, command, destination, parse_stats_line, percent,
    };

    #[test]
    fn parses_json_log_stats_lines() {
        let line = r#"{"level":"info","msg":"...","stats":{"bytes":512,"totalBytes":2048,"transferring":[{"name":"a.bin"}]},"time":"2026-01-01T00:00:00Z"}"#;
        assert_eq!(parse_stats_line(line), Some((512, Some(2048))));
        assert_eq!(percent(512, Some(2048)), Some(25));
        assert_eq!(parse_stats_line(r#"{"level":"error","msg":"boom"}"#), None);
        assert_eq!(parse_stats_line("plain text"), None);
        // Unknown totals must not break the parse.
        assert_eq!(
            parse_stats_line(r#"{"stats":{"bytes":7}}"#),
            Some((7, None))
        );
        assert_eq!(percent(7, None), None);
        assert_eq!(percent(7, Some(0)), None);
    }

    #[test]
    fn destination_keeps_the_remote_and_sanitizes_the_package() {
        assert_eq!(
            destination("gdrive:downloads/", "My: Release?"),
            "gdrive:downloads/My_ Release_"
        );
        assert_eq!(UploadMode::from_setting("MOVE"), UploadMode::Move);
        assert_eq!(UploadMode::from_setting("weird"), UploadMode::Copy);
    }

    fn arguments(bwlimit: Option<u64>) -> Vec<String> {
        args_of(
            UploadMode::Copy,
            std::path::Path::new("/downloads/Release"),
            bwlimit,
            "gdrive:downloads/Release",
        )
    }

    fn args_of(
        mode: UploadMode,
        directory: &std::path::Path,
        bwlimit: Option<u64>,
        target: &str,
    ) -> Vec<String> {
        let context = UploadContext {
            remote: "gdrive:downloads",
            mode,
            package_name: "Release",
            directory,
            executable: None,
            vendor_directory: None,
            bwlimit,
        };
        command(std::path::Path::new("rclone"), &context, target)
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_upload_limit_reaches_rclone_as_bwlimit_in_bytes() {
        let arguments = arguments(Some(1_500_000));
        let at = arguments
            .iter()
            .position(|argument| argument == "--bwlimit")
            .expect("--bwlimit is passed");
        assert_eq!(arguments[at + 1], "1500000B");
        // A flag, so it has to come before the `--` that ends them.
        let end = arguments
            .iter()
            .position(|argument| argument == "--")
            .expect("--");
        assert!(at < end, "{arguments:?}");
    }

    #[test]
    fn an_unlimited_upload_passes_no_bwlimit() {
        assert!(
            !arguments(None)
                .iter()
                .any(|argument| argument == "--bwlimit")
        );
        assert_eq!(bwlimit(Some(0)), None);
    }

    #[test]
    fn every_flag_comes_before_the_paths() {
        let directory = std::path::Path::new("-release");
        let moved = args_of(UploadMode::Move, directory, None, "gdrive:-x");
        let separator = moved.iter().position(|a| a == "--").expect("separator");
        assert_eq!(moved[separator + 1..], ["-release", "gdrive:-x"]);
        assert!(
            moved[..separator]
                .iter()
                .any(|a| a == "--delete-empty-src-dirs")
        );
        assert_eq!(moved[0], "move");
        let copied = args_of(UploadMode::Copy, directory, None, "gdrive:x");
        assert!(!copied.iter().any(|a| a == "--delete-empty-src-dirs"));
        assert_eq!(copied[0], "copy");
    }
}
