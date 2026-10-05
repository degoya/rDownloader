//! Direct unpack (RD-1100-07): a RAR set unpacked while its volumes are still arriving.
//!
//! `unrar` started with `-vp` stops before every volume after the first and asks on the console
//! whether to go on — `Insert disk with <volume>`, then `[C]ontinue, [Q]uit`. [`extract_rar_direct`]
//! hands that question to its caller, which answers once the volume is on disk and intact, or
//! says no, and the tool is stopped. SABnzbd's `directunpacker` drives `unrar` the same way.
//!
//! Everything goes into a staging directory of its own inside the package folder, named with
//! [`DIRECT_STAGING_PREFIX`]; nothing reaches the package until [`adopt_direct`] moves it there.
//! 7-Zip cannot pause between volumes, so it is refused before anything starts.

use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

use crate::{
    ArchiveLimits, ExternalRarTool, ExtractionError, ExtractionReport, RarToolKind,
    archive::{merge_tree, validate_tree},
    rar::{
        OUTPUT_TAIL, RarProcess, SIZE_POLL_INTERVAL, exit_text, log_finished, merged_output,
        spawn_rar, watch_staging_size,
    },
    rar_args::RarAction,
    rar_exit::classify_failure,
};

/// What every direct unpack's staging directory is named with.
///
/// It starts with [`crate::STAGING_PREFIX`], so every walk that already leaves a staging
/// directory alone — the plugin steps' file list, the upload, the cleanup — leaves this one alone
/// too, and an extraction into the package folder removes one a crash left behind.
pub const DIRECT_STAGING_PREFIX: &str = ".rd-xd";

/// What `unrar` prints in front of the volume it waits for.
const ASK_PREFIX: &str = "Insert disk with ";

/// The choice the question ends with; only then has the whole question been printed.
const ASK_CHOICE: &str = "[C]ontinue";

/// How much of the console the volume question is looked for in.
const PROMPT_WINDOW: usize = 4 * 1024;

/// One volume the running tool waits for.
#[derive(Debug)]
pub struct VolumeAsk {
    /// The volume's file name as the tool named it, without its folder.
    pub volume: String,
    /// `true` goes on with that volume, `false` stops the tool; a dropped sender is `false`.
    pub reply: oneshot::Sender<bool>,
}

/// One direct unpack.
pub struct DirectRequest<'a> {
    pub tool: &'a ExternalRarTool,
    pub first_volume: &'a Path,
    /// The package folder. The staging directory is created inside it, on the filesystem the
    /// output is moved to later.
    pub parent: &'a Path,
    pub limits: ArchiveLimits,
    pub password: Option<&'a str>,
    /// Where the volume questions go.
    pub asks: mpsc::Sender<VolumeAsk>,
}

/// A finished direct unpack: where its validated tree waits, and which volumes it read.
#[derive(Clone, Debug)]
pub struct DirectStaging {
    pub staging: PathBuf,
    pub report: ExtractionReport,
    /// The file names of the volumes the tool read, the first one included, in order.
    pub volumes: Vec<String>,
}

/// Unpacks the RAR set that starts at `first_volume`, asking for every further volume.
///
/// On success the tree is validated against the limits, as an ordinary unpack's is, and left in
/// its staging directory for [`adopt_direct`]. On any failure — the tool's verdict, a question
/// answered with no, the size limit, the timeout — the staging directory is removed again.
///
/// The tool's timeout covers the work on one volume, not the wait for the next: a download may
/// take hours to bring it, and the tool does nothing all that time.
///
/// # Errors
///
/// [`ExtractionError::Unsupported`] for a tool other than `unrar`; otherwise what the tool or
/// the validation reported, or [`ExtractionError::Other`] for a stop.
pub async fn extract_rar_direct(
    request: DirectRequest<'_>,
) -> Result<DirectStaging, ExtractionError> {
    if request.tool.kind != RarToolKind::Unrar {
        return Err(ExtractionError::Unsupported(
            "direct unpack needs unrar; 7-Zip cannot pause between volumes".to_owned(),
        ));
    }
    let staging = rd_files::long_path(
        &tempfile::Builder::new()
            .prefix(DIRECT_STAGING_PREFIX)
            .tempdir_in(request.parent)?
            .keep(),
    );
    match follow(&request, &staging).await {
        Ok((report, volumes)) => Ok(DirectStaging {
            staging,
            report,
            volumes,
        }),
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            Err(error)
        }
    }
}

/// Moves a finished direct unpack's tree into `destination` and removes its staging directory.
///
/// The same merge an ordinary unpack ends with: same-named files are replaced, same-named
/// folders merged.
///
/// # Errors
///
/// A move that failed; what was moved before it stays in `destination`, the rest in staging.
pub fn adopt_direct(staging: &Path, destination: &Path) -> anyhow::Result<()> {
    merge_tree(staging, &rd_files::long_path(destination))?;
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

/// Runs the tool, answering its volume questions through `request.asks`.
async fn follow(
    request: &DirectRequest<'_>,
    staging: &Path,
) -> Result<(ExtractionReport, Vec<String>), ExtractionError> {
    let tool = request.tool;
    let RarProcess {
        mut child,
        mut stdout,
        mut stderr,
        stdin,
        arguments,
    } = spawn_rar(
        tool,
        RarAction::Follow { staging },
        request.first_volume,
        request.password,
    )?;
    let mut stdin = stdin.context("RAR tool stdin")?;
    let mut volumes = vec![file_name(request.first_volume)];
    let mut console = String::new();
    let mut stdout_tail = String::new();
    let mut stderr_tail = String::new();
    let mut stdout_buffer = [0_u8; 512];
    let mut stderr_buffer = [0_u8; 512];
    let (mut stdout_open, mut stderr_open) = (true, true);
    let watcher = watch_staging_size(
        staging,
        request.limits.max_uncompressed_bytes,
        SIZE_POLL_INTERVAL,
    );
    tokio::pin!(watcher);
    let mut deadline = tokio::time::Instant::now() + tool.timeout;
    while stdout_open || stderr_open {
        // `unrar` writes the question to stderr and the file list to stdout; both are read, and
        // the question is looked for in what they printed together.
        let (chunk, from_stderr) = tokio::select! {
            read = stdout.read(&mut stdout_buffer), if stdout_open => {
                let read = read.context("read RAR tool output")?;
                if read == 0 {
                    stdout_open = false;
                    continue;
                }
                (String::from_utf8_lossy(&stdout_buffer[..read]).into_owned(), false)
            }
            read = stderr.read(&mut stderr_buffer), if stderr_open => {
                let read = read.context("read RAR tool output")?;
                if read == 0 {
                    stderr_open = false;
                    continue;
                }
                (String::from_utf8_lossy(&stderr_buffer[..read]).into_owned(), true)
            }
            () = &mut watcher => {
                let _ = child.kill().await;
                log_finished(tool, &arguments, "killed: uncompressed-size limit");
                // Word for word what an ordinary unpack reports, so the code stays the same.
                return Err(ExtractionError::Other(anyhow::anyhow!(
                    "archive exceeds uncompressed-size limit"
                )));
            }
            () = tokio::time::sleep_until(deadline) => {
                let _ = child.kill().await;
                log_finished(tool, &arguments, "killed: timed out");
                return Err(ExtractionError::Other(anyhow::anyhow!(
                    "RAR extraction timed out"
                )));
            }
        };
        let tail = if from_stderr {
            &mut stderr_tail
        } else {
            &mut stdout_tail
        };
        tail.push_str(&chunk);
        keep_tail(tail, OUTPUT_TAIL);
        console.push_str(&chunk);
        keep_tail(&mut console, PROMPT_WINDOW);
        let Some(volume) = asked_volume(&console) else {
            continue;
        };
        console.clear();
        // Asked again for the volume it was just given: the tool could not use it, and asking
        // a third time would not change that.
        let repeated = volumes.len() > 1 && volumes.last() == Some(&volume);
        let (reply, answer) = oneshot::channel();
        let go_on = !repeated
            && request
                .asks
                .send(VolumeAsk {
                    volume: volume.clone(),
                    reply,
                })
                .await
                .is_ok()
            && answer.await.unwrap_or(false);
        if !go_on {
            let _ = child.kill().await;
            log_finished(tool, &arguments, "killed: direct unpack abandoned");
            return Err(ExtractionError::Other(anyhow::anyhow!(
                "direct unpack abandoned before {volume}"
            )));
        }
        stdin
            .write_all(b"C\n")
            .await
            .context("answer the RAR tool")?;
        stdin.flush().await.context("answer the RAR tool")?;
        volumes.push(volume);
        deadline = tokio::time::Instant::now() + tool.timeout;
    }
    drop(stdin);
    let status = match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(status) => status.context("run RAR tool")?,
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
            &merged_output(&stdout_tail, stderr_tail.as_bytes()),
            request.password,
        ));
    }
    let root = staging.to_owned();
    let limits = request.limits;
    let report = tokio::task::spawn_blocking(move || validate_tree(&root, limits))
        .await
        .context("join RAR validation")??;
    Ok((report, volumes))
}

/// The volume a `-vp` pause asks for, once the whole question has been printed.
///
/// `unrar` names the volume with the folder it computed it in; only the file name is kept, so
/// the caller compares it with what it knows of the package.
pub(crate) fn asked_volume(console: &str) -> Option<String> {
    let start = console.rfind(ASK_PREFIX)? + ASK_PREFIX.len();
    let rest = &console[start..];
    let choice = rest.find(ASK_CHOICE)?;
    let line = rest[..choice].lines().next()?.trim();
    let name = line.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Shortens `text` to about its last `limit` bytes, never inside a character.
pub(crate) fn keep_tail(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut cut = text.len() - limit;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
