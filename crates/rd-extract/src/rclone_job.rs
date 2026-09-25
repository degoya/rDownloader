//! rclone upload of a finished package folder to a configured remote
//! (`remote:path/<package>`), with progress parsed from rclone's JSON log.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState};
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
}

/// `remote:path/<sanitized package name>` — the package keeps its own folder remotely.
pub(crate) fn destination(remote: &str, package_name: &str) -> String {
    format!(
        "{}/{}",
        remote.trim_end_matches('/'),
        rd_files::sanitize_file_name(package_name)
    )
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
    let mut command = tokio::process::Command::new(&tool.path);
    command
        .arg(context.mode.verb())
        .args(["--use-json-log", "--stats", "1s", "-v"])
        // `--` before the positionals, as every other external invocation in the tree does:
        // a configured remote or a package directory beginning with `-` is otherwise read by
        // rclone as a flag rather than as a path.
        .arg("--")
        .arg(context.directory)
        .arg(target);
    if context.mode == UploadMode::Move {
        command.arg("--delete-empty-src-dirs");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
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
    use super::{UploadMode, destination, parse_stats_line, percent};

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
}
