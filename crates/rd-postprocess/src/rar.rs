use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use tokio::io::AsyncReadExt;

use crate::{
    ArchiveLimits, ExtractionError, ExtractionReport, ProgressSender,
    archive::validate_tree,
    progress::{ExtractProgress, parse_tool_percent, report},
    rar_args::{RarAction, RarArguments, rar_arguments},
    rar_exit::classify_failure,
    tool_env::apply_tool_environment,
};

/// Supported user-supplied RAR programs; neither is linked into rDownloader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RarToolKind {
    Unrar,
    SevenZip,
}

/// Explicit executable and timeout without shell invocation.
#[derive(Clone, Debug)]
pub struct ExternalRarTool {
    pub executable: PathBuf,
    pub kind: RarToolKind,
    pub timeout: Duration,
}

/// Extracts a RAR set (first volume) into `staging` and validates the resulting tree.
/// The password is passed on the command line (`-p`), which is what both tools expect; only
/// [`RarArguments::redacted`] is ever logged. Console output is streamed to derive the percentage.
///
/// The process locale is set explicitly (`tool_env`): `unrar` decodes that `-p` argument through
/// it, so a service without `LANG` would reject a correct non-ASCII password (RD-107-11).
pub(crate) async fn extract_rar_into(
    tool: &ExternalRarTool,
    first_volume: &Path,
    staging: &Path,
    limits: ArchiveLimits,
    password: Option<&str>,
    progress: Option<&ProgressSender>,
) -> Result<ExtractionReport, ExtractionError> {
    if !tool.executable.exists() {
        return Err(ExtractionError::Other(anyhow::anyhow!(
            "configured RAR executable does not exist"
        )));
    }
    let mut command = tokio::process::Command::new(&tool.executable);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_tool_environment(&mut command);
    let arguments = rar_arguments(
        tool.kind,
        RarAction::Extract { staging },
        first_volume,
        password,
    );
    arguments.apply_to(tool.kind, &mut command)?;
    log_started(tool, &arguments);
    let mut child = command.spawn().context("spawn RAR tool")?;
    let mut stdout = child.stdout.take().context("RAR tool stdout")?;
    let mut stderr = child.stderr.take().context("RAR tool stderr")?;
    let run = async {
        let mut stdout_text = String::new();
        let mut stderr_buffer = Vec::new();
        let reader = async {
            let mut buffer = [0_u8; 512];
            loop {
                let read = stdout.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buffer[..read]);
                if let Some(percent) = parse_tool_percent(&chunk) {
                    report(
                        progress,
                        ExtractProgress {
                            done_bytes: 0,
                            total_bytes: None,
                            percent: Some(percent),
                            current: None,
                        },
                    );
                }
                if stdout_text.len() < 16 * 1024 {
                    stdout_text.push_str(&chunk);
                }
            }
            Ok::<_, std::io::Error>(())
        };
        let (read_result, stderr_result) =
            tokio::join!(reader, stderr.read_to_end(&mut stderr_buffer));
        read_result?;
        stderr_result?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, stdout_text, stderr_buffer))
    };
    // `None` means the watcher fired: the staging tree passed the uncompressed-size limit while
    // the tool was still writing. Both futures are dropped with the select, which releases the
    // borrow on `child` - the timeout arm below has always relied on that - so the child is
    // killed the same way either way.
    let finished = tokio::select! {
        finished = tokio::time::timeout(tool.timeout, run) => Some(finished),
        () = watch_staging_size(staging, limits.max_uncompressed_bytes, SIZE_POLL_INTERVAL) => None,
    };
    let Some(finished) = finished else {
        let _ = child.kill().await;
        log_finished(tool, &arguments, "killed: uncompressed-size limit");
        // Deliberately the same failure `validate_tree` reports for an oversized tree, word for
        // word: the step keeps its stable `extract.failed` code and its detail, so nothing on the
        // way to the user has to learn that the limit can now be hit earlier.
        return Err(ExtractionError::Other(anyhow::anyhow!(
            "archive exceeds uncompressed-size limit"
        )));
    };
    let (status, stdout_text, stderr_buffer) = match finished {
        Ok(result) => result.context("run RAR tool")?,
        Err(_) => {
            let _ = child.kill().await;
            log_finished(tool, &arguments, "killed: timed out");
            return Err(ExtractionError::Other(anyhow::anyhow!(
                "RAR extraction timed out"
            )));
        }
    };
    log_finished(tool, &arguments, &exit_text(status));
    if !status.success() {
        return Err(classify_failure(
            tool.kind,
            status.code(),
            &merged_output(&stdout_text, &stderr_buffer),
            password,
        ));
    }
    let staging = staging.to_owned();
    Ok(
        tokio::task::spawn_blocking(move || validate_tree(&staging, limits))
            .await
            .context("join RAR validation")??,
    )
}

/// How long the size watcher waits between two measurements of the staging tree: two seconds.
///
/// The overshoot this allows is the interval times the tool's write rate - a couple of hundred
/// megabytes past `max_uncompressed_bytes` on fast local storage - which the volume survives,
/// whereas the bomb running to completion or to `tool.timeout` is what fills it. Measuring more
/// often would not buy much and would cost more than it looks: every walk `stat`s every file
/// already written, up to `max_files` of them, on the same directories the unpack is writing to.
const SIZE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Resolves once the staging tree has grown past `limit`; otherwise it waits forever.
///
/// `validate_tree` enforces the same limit, but only once the tool has stopped writing - by
/// which time a decompression bomb has already filled the download volume, and deleting the
/// staging directory afterwards does not give those minutes back. The external tools offer no
/// output budget of their own, so the only brake on the bytes as they land is to watch them and
/// kill the process; the in-process ZIP and 7z readers do the same thing by counting the
/// declared size of each member before they write it.
///
/// Sleeping between walks rather than ticking on a schedule keeps a slow walk over a large tree
/// from degenerating into a continuous one: the gap is always honoured.
async fn watch_staging_size(staging: &Path, limit: u64, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        let root = staging.to_owned();
        // Blocking file I/O beside a running unpack, so it goes where `validate_tree` goes.
        // A join error means the walk itself panicked or was cancelled; treating that as "not
        // over the limit" leaves `tool.timeout` as the brake rather than killing a healthy
        // unpack on a measurement that never happened.
        let exceeded = tokio::task::spawn_blocking(move || tree_exceeds(&root, limit))
            .await
            .unwrap_or(false);
        if exceeded {
            return;
        }
    }
}

/// Whether the files below `root` already add up to more than `limit`, stopping as soon as they do.
///
/// Reading errors answer "no": the tree is being written while it is walked, so a directory or a
/// file can disappear between the two calls, and a vanished entry is not evidence of a bomb.
/// `validate_tree` is still the authority once the tool has finished - this only has to be right
/// about the one case where the tree keeps growing.
///
/// Symbolic links are neither followed nor counted. Following one would measure a tree outside
/// the staging directory, and `validate_tree` rejects the link itself afterwards anyway.
fn tree_exceeds(root: &Path, limit: u64) -> bool {
    let mut directories = vec![root.to_owned()];
    let mut total = 0_u64;
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
                if total > limit {
                    return true;
                }
            }
        }
    }
    false
}

/// The tool, and its arguments without the password, before it starts (RD-120-56).
///
/// Until then nothing of an extraction reached the log, so a failure that only happens with one
/// command line - the doubled separator under Windows - could not be seen from the log at all.
fn log_started(tool: &ExternalRarTool, arguments: &RarArguments) {
    tracing::info!(
        tool = %tool.executable.display(),
        args = %arguments.redacted(),
        "RAR tool started"
    );
}

/// The same line again with the outcome: the exit code, or why the process was killed.
fn log_finished(tool: &ExternalRarTool, arguments: &RarArguments, exit: &str) {
    tracing::info!(
        tool = %tool.executable.display(),
        args = %arguments.redacted(),
        exit,
        "RAR tool finished"
    );
}

fn exit_text(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string())
}

/// stderr first, then stdout, lowercased once for the classifier.
///
/// Both tools split their reporting: `unrar` writes the diagnosis to stderr and the progress to
/// stdout, 7-Zip the other way round depending on `-bso`/`-bse`.
fn merged_output(stdout_text: &str, stderr_buffer: &[u8]) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(stderr_buffer).to_lowercase(),
        stdout_text.to_lowercase()
    )
}

/// Tests a RAR set without extracting it (`unrar t` / `7z t`).
///
/// SABnzbd's `try_rar_check`: when no PAR2 set answered whether the payload arrived intact,
/// the archive itself is asked. It reads every volume and verifies the stored CRCs, so it
/// costs a pass over the data but writes nothing — which is exactly what is wanted before
/// deciding whether an unpack is worth attempting (RD-104-04).
pub async fn test_rar(
    tool: &ExternalRarTool,
    first_volume: &Path,
    password: Option<&str>,
) -> Result<(), ExtractionError> {
    if !tool.executable.exists() {
        return Err(ExtractionError::Other(anyhow::anyhow!(
            "configured RAR executable does not exist"
        )));
    }
    let mut command = tokio::process::Command::new(&tool.executable);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_tool_environment(&mut command);
    let arguments = rar_arguments(tool.kind, RarAction::Test, first_volume, password);
    arguments.apply_to(tool.kind, &mut command)?;
    log_started(tool, &arguments);
    let mut child = command.spawn().context("spawn RAR tool")?;
    let mut stdout = child.stdout.take().context("RAR tool stdout")?;
    let mut stderr = child.stderr.take().context("RAR tool stderr")?;
    let run = async {
        let mut stdout_buffer = Vec::new();
        let mut stderr_buffer = Vec::new();
        let (out, err) = tokio::join!(
            stdout.read_to_end(&mut stdout_buffer),
            stderr.read_to_end(&mut stderr_buffer)
        );
        out?;
        err?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, stdout_buffer, stderr_buffer))
    };
    let (status, stdout_buffer, stderr_buffer) = match tokio::time::timeout(tool.timeout, run).await
    {
        Ok(result) => result.context("run RAR tool")?,
        Err(_) => {
            let _ = child.kill().await;
            log_finished(tool, &arguments, "killed: timed out");
            return Err(ExtractionError::Other(anyhow::anyhow!(
                "RAR integrity test timed out"
            )));
        }
    };
    log_finished(tool, &arguments, &exit_text(status));
    if status.success() {
        return Ok(());
    }
    Err(classify_failure(
        tool.kind,
        status.code(),
        &merged_output(&String::from_utf8_lossy(&stdout_buffer), &stderr_buffer),
        password,
    ))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use super::{tree_exceeds, watch_staging_size};

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().expect("parent directory")).expect("directories");
        std::fs::write(path, vec![0_u8; bytes]).expect("file");
    }

    #[test]
    fn the_measurement_covers_the_whole_tree_and_an_unreadable_one_is_not_a_bomb() {
        let temp = tempfile::tempdir().expect("temporary directory");
        write(&temp.path().join("a.bin"), 600);
        write(&temp.path().join("nested/b.bin"), 600);
        assert!(tree_exceeds(temp.path(), 1_000));
        assert!(!tree_exceeds(temp.path(), 2_000));
        // A directory that is not there yet, or no longer, says nothing about the size.
        assert!(!tree_exceeds(&temp.path().join("gone"), 0));
    }

    /// The limit has to act while the tool is still writing: `validate_tree` runs only once it
    /// has stopped, and by then a RAR bomb has filled the volume.
    #[tokio::test]
    async fn the_watcher_waits_below_the_limit_and_fires_above_it() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let interval = Duration::from_millis(10);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(150),
                watch_staging_size(temp.path(), 1_000, interval),
            )
            .await
            .is_err(),
            "the watcher fired on a tree well below the limit"
        );
        write(&temp.path().join("bomb/large.bin"), 4_096);
        tokio::time::timeout(
            Duration::from_secs(5),
            watch_staging_size(temp.path(), 1_000, interval),
        )
        .await
        .expect("the watcher did not notice the oversized tree");
    }
}
