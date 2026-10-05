use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use rd_files::NoConsoleWindow as _;
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
    let RarProcess {
        mut child,
        mut stdout,
        mut stderr,
        arguments,
        ..
    } = spawn_rar(tool, RarAction::Extract { staging }, first_volume, password)?;
    let run = async {
        let mut stdout_text = String::new();
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
        let (read_result, stderr_buffer) =
            tokio::join!(reader, rd_files::read_tail(&mut stderr, OUTPUT_TAIL));
        read_result?;
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

/// How much of a RAR tool's stderr, and of `t`'s listing, is kept for the verdict: the last
/// 64 KiB. The messages `classify_failure` looks for are the tool's last words.
pub(crate) const OUTPUT_TAIL: usize = 64 * 1024;

/// How long the size watcher waits between two measurements of the staging tree: two seconds.
///
/// The overshoot this allows is the interval times the tool's write rate - a couple of hundred
/// megabytes past `max_uncompressed_bytes` on fast local storage - which the volume survives,
/// whereas the bomb running to completion or to `tool.timeout` is what fills it. Measuring more
/// often would not buy much and would cost more than it looks: every walk `stat`s every file
/// already written, up to `max_files` of them, on the same directories the unpack is writing to.
pub(crate) const SIZE_POLL_INTERVAL: Duration = Duration::from_secs(2);

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
pub(crate) async fn watch_staging_size(staging: &Path, limit: u64, interval: Duration) {
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

/// A started RAR tool, its output taken, and the arguments it was started with for the log.
pub(crate) struct RarProcess {
    pub(crate) child: tokio::process::Child,
    pub(crate) stdout: tokio::process::ChildStdout,
    pub(crate) stderr: tokio::process::ChildStderr,
    /// Only for [`RarAction::Follow`], which is answered on it; every other action gets none.
    pub(crate) stdin: Option<tokio::process::ChildStdin>,
    pub(crate) arguments: RarArguments,
}

/// Starts the RAR tool for `action` the one way both the unpack and the integrity test do: no
/// shell, no console window, the tool environment, no stdin, piped output, killed when dropped
/// (audit 1.9.1, INTAKE-15). Direct unpack's [`RarAction::Follow`] is the one action with a
/// stdin, because its volume questions are answered there (RD-1100-07).
pub(crate) fn spawn_rar(
    tool: &ExternalRarTool,
    action: RarAction<'_>,
    first_volume: &Path,
    password: Option<&str>,
) -> Result<RarProcess, ExtractionError> {
    if !tool.executable.exists() {
        return Err(ExtractionError::Other(anyhow::anyhow!(
            "configured RAR executable does not exist"
        )));
    }
    let mut command = tokio::process::Command::new(&tool.executable);
    let answered = matches!(action, RarAction::Follow { .. });
    command
        .kill_on_drop(true)
        .stdin(if answered {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_console_window();
    apply_tool_environment(&mut command);
    let arguments = rar_arguments(tool.kind, action, first_volume, password);
    arguments
        .apply_to(tool.kind, &mut command)
        .inspect_err(|error| log_refused(tool, &arguments, error))?;
    log_started(tool, &arguments);
    let mut child = command.spawn().context("spawn RAR tool")?;
    let stdout = child.stdout.take().context("RAR tool stdout")?;
    let stderr = child.stderr.take().context("RAR tool stderr")?;
    let stdin = child.stdin.take();
    Ok(RarProcess {
        child,
        stdout,
        stderr,
        stdin,
        arguments,
    })
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

/// An argument list the tool's command line cannot carry, refused before the tool starts - a
/// 7-Zip password with a `"` under Windows, or a NUL. Only the code is logged, and the arguments
/// without the password, like every other line here.
fn log_refused(tool: &ExternalRarTool, arguments: &RarArguments, error: &ExtractionError) {
    tracing::warn!(
        tool = %tool.executable.display(),
        args = %arguments.redacted(),
        code = error.code(),
        "RAR tool not started"
    );
}

/// The same line again with the outcome: the exit code, or why the process was killed.
pub(crate) fn log_finished(tool: &ExternalRarTool, arguments: &RarArguments, exit: &str) {
    tracing::info!(
        tool = %tool.executable.display(),
        args = %arguments.redacted(),
        exit,
        "RAR tool finished"
    );
}

pub(crate) fn exit_text(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string())
}

/// stderr first, then stdout, lowercased once for the classifier.
///
/// Both tools split their reporting: `unrar` writes the diagnosis to stderr and the progress to
/// stdout, 7-Zip the other way round depending on `-bso`/`-bse`.
pub(crate) fn merged_output(stdout_text: &str, stderr_buffer: &[u8]) -> String {
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
    let RarProcess {
        mut child,
        mut stdout,
        mut stderr,
        arguments,
        ..
    } = spawn_rar(tool, RarAction::Test, first_volume, password)?;
    let run = async {
        // `t` lists every member it tested, so a large set prints a line per file; only the end,
        // where the verdict is, is kept.
        let (stdout_buffer, stderr_buffer) = tokio::join!(
            rd_files::read_tail(&mut stdout, OUTPUT_TAIL),
            rd_files::read_tail(&mut stderr, OUTPUT_TAIL)
        );
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
