//! The scaffolding every external-tool run shares (RD-108-31).
//!
//! `rd-media`, `rd-gallery` and `rd-stream` each start a long-running external process and
//! follow it until it ends. Three copies of the same preparation grew apart: the lease and the
//! compatibility gate were ordered differently, one copy lost the comment saying *why* they are
//! ordered that way, and the Windows console-window flag, the `kill_on_drop` and the detached
//! stderr drain were re-derived each time. A drift in that scaffolding is invisible — a missing
//! `kill_on_drop` leaks a process, a missing creation flag pops a console window on Windows,
//! and a missing lease lets an activation delete the binary a running job is executing — so it
//! belongs in one place.
//!
//! This crate is the home because it already owns both halves of the preparation: the lease
//! ([`crate::lease`]) and the compatibility verdict ([`crate::compat`]). All three callers
//! already depend on it, and it depends on none of them.
//!
//! What is *not* here: what the loop does with a line. Each runner reads a different thing out
//! of its process — yt-dlp's progress lines, gallery-dl's stored paths, streamlink's growing
//! file — and folding those into one callback would hide the differences rather than remove
//! them. The reading itself is here ([`ToolProcess::next_line`]): the select between the next
//! line, a stop and a time limit was the same in every copy (audit 1.9.1, TR-13).
//!
//! There are two shapes, not one. [`ToolProcess`] is the long-running download that is followed
//! until it ends; [`run_to_output`] is the short question — `yt-dlp -J`, `streamlink --json`,
//! `<tool> --version` — that is asked under a timeout and answered in seconds. They share the
//! stdin, `kill_on_drop` and console-window wiring and nothing else, so they are two functions
//! over one set of rules rather than one function with a mode flag.

use std::{future::Future, path::PathBuf, process::Output, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use rd_core::{Failure, FailureKind, ToolLease};
use rd_files::NoConsoleWindow as _;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout, Command},
    time::{Instant, error::Elapsed},
};

/// How often a running external tool's progress is written to the queue row.
///
/// Every write is a serialized-writer round trip, and a tool that reports ten lines a second
/// would otherwise turn one download into a write storm.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(750);

/// How much of a running tool's stderr is kept: its last 64 KiB.
pub const STDERR_TAIL: usize = 64 * 1024;

/// How long a download tool may go without a line on stdout before it counts as hung.
///
/// yt-dlp prints a progress line every second while it downloads, but nothing while ffmpeg
/// merges or converts afterwards; gallery-dl prints one path per stored file, so a large file
/// is silence for as long as it takes. Half an hour leaves both room; a tool that is quiet
/// longer is killed and retried, and both resume their partial files (audit 2026-10-08, TR-05).
pub const SILENCE_LIMIT: Duration = Duration::from_secs(30 * 60);

/// An external tool that was located, leased and found compatible.
///
/// The lease is the point of the type: it is never read, only held, and dropping this value is
/// what releases it. A managed version is not removed while a lease on it is alive, so the
/// binary a running job is executing survives another version being activated underneath it
/// (RD-102-02).
#[derive(Debug)]
pub struct PreparedTool {
    path: PathBuf,
    #[allow(dead_code)]
    lease: Option<ToolLease>,
}

impl PreparedTool {
    /// Where the binary is.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// Leases a located tool and gates it on the compatibility rules for one capability.
///
/// `located` is what the caller's own lookup found, because the lookups genuinely differ: the
/// recorder also accepts a portable Windows layout that `locate_tool_leased` does not know
/// about. What follows the lookup is the same everywhere and is done here.
///
/// **The lease is taken before the verdict is asked for, and that order is deliberate.**
/// Assessing a version runs the binary, so a version activated in between would otherwise let
/// the store delete the file between the assessment and the spawn — and the job would then run
/// a binary nobody assessed, or none at all. Only the named capability is gated; a yt-dlp below
/// the floor stops media downloads and leaves the rest of the queue alone (RD-102-03).
///
/// The `Err` is the [`Failure`] the runner reports verbatim: the stable codes
/// `media.tool_missing` and `media.tool_incompatible`, both of which the web client translates.
pub async fn prepare(
    name: &str,
    located: Option<(PathBuf, Option<ToolLease>)>,
    capability: crate::Capability,
) -> Result<PreparedTool, Failure> {
    let Some((path, lease)) = located else {
        return Err(Failure::coded(
            FailureKind::Unsupported,
            "media.tool_missing",
            format!("{name} is not installed or not configured"),
        )
        .with_param("tool", name));
    };
    let assessment = crate::compat::assess(name, &path).await;
    if assessment.blocks(capability) {
        return Err(crate::compat::incompatible_failure(&assessment, capability));
    }
    Ok(PreparedTool { path, lease })
}

/// Whether a spawned tool's stdout is read or thrown away.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stdout {
    /// The runner parses stdout: progress lines, stored paths, the final file name.
    Read,
    /// Nothing parses stdout, so it goes to the null device.
    ///
    /// Not merely unread: an unread pipe fills its buffer and then blocks the tool forever.
    Discarded,
}

/// A spawned external tool, with its stderr already being drained.
///
/// stdin is always the null device — a tool that decides to prompt must fail rather than wait
/// for a keystroke nobody will type — and `kill_on_drop` is always on, so an aborted task takes
/// the process with it instead of leaving an orphan writing into the download folder.
pub struct ToolProcess {
    child: Child,
    stdout: Option<ChildStdout>,
    lines: Option<BufReader<ChildStdout>>,
    stderr: tokio::task::JoinHandle<String>,
    deadline: Option<Instant>,
    silence: Option<Duration>,
}

/// What [`ToolProcess::next_line`] found.
#[derive(Debug, Eq, PartialEq)]
pub enum ToolLine {
    /// One line of stdout, without its line ending; bytes that are not UTF-8 are replaced.
    Line(String),
    /// stdout is closed: the tool is done talking, [`ToolProcess::wait`] tells how it ended.
    End,
    /// The stop the caller handed in fired first; the process is killed.
    Stopped,
    /// The time limit of [`ToolProcess::with_deadline`] or the silence limit of
    /// [`ToolProcess::with_silence_limit`] passed first; the process is killed.
    TimedOut,
}

/// How [`ToolProcess::wait_or_stop`] ended.
#[derive(Debug)]
pub enum ToolEnd {
    /// The process exited by itself.
    Exited(std::process::ExitStatus),
    /// The stop the caller handed in fired first; the process is killed.
    Stopped,
    /// The time limit of [`ToolProcess::with_deadline`] passed first; the process is killed.
    TimedOut,
}

impl ToolProcess {
    /// Applies the shared stdio wiring to `command` and starts it.
    ///
    /// `name` appears only in the error context, so the caller reads "spawn yt-dlp" rather
    /// than a bare `io::Error` with no indication of which of the four tools failed.
    ///
    /// stderr is drained by a detached task from the moment the process exists. Reading it only
    /// after the child has ended would deadlock the tool the first time it writes more than a
    /// pipe buffer of warnings — which yt-dlp does on any long download.
    ///
    /// The environment is the allowlist of [`rd_files::restrict_environment`] plus
    /// [`rd_files::TOOL_VARIABLES`] and whatever the caller set: none of these tools needs the
    /// service's own variables, and a credential an operator keeps there is not theirs to read.
    ///
    /// Only the last [`STDERR_TAIL`] bytes of stderr are kept; the rest is read and dropped.
    pub fn spawn(command: &mut Command, name: &str, stdout: Stdout) -> Result<Self> {
        rd_files::restrict_environment(command, rd_files::TOOL_VARIABLES);
        speak_utf8(command);
        crate::workdir::isolate(command);
        command
            .stdin(Stdio::null())
            .stdout(match stdout {
                Stdout::Read => Stdio::piped(),
                Stdout::Discarded => Stdio::null(),
            })
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .no_console_window();
        let mut child = command.spawn().with_context(|| format!("spawn {name}"))?;
        let stdout = child.stdout.take();
        let handle = child
            .stderr
            .take()
            .with_context(|| format!("{name} stderr"))?;
        // Bounded: a recording that runs for hours can warn about every segment it retries, and
        // a buffer that kept all of it grew the service with it. The errors the runners look
        // for are the last thing a tool prints.
        let stderr = tokio::spawn(async move {
            String::from_utf8_lossy(&rd_files::read_tail(handle, STDERR_TAIL).await).into_owned()
        });
        Ok(Self {
            child,
            stdout,
            lines: None,
            stderr,
            deadline: None,
            silence: None,
        })
    }

    /// Gives the run a time limit, counted from now: once it has passed, [`Self::next_line`] and
    /// [`Self::wait_or_stop`] kill the process and answer `TimedOut`. A tool that hangs — a
    /// network share that stopped answering, a prompt nobody sees — otherwise holds its job
    /// and its slot for good.
    #[must_use]
    pub fn with_deadline(mut self, limit: Duration) -> Self {
        self.deadline = Some(Instant::now() + limit);
        self
    }

    /// Gives the run a silence limit: once [`Self::next_line`] has waited that long without a
    /// line, it kills the process and answers `TimedOut`. Unlike a deadline it does not end a
    /// run that keeps talking, however long it takes; it ends one that has hung. It applies to
    /// the reading of stdout only, not to [`Self::wait_or_stop`].
    #[must_use]
    pub fn with_silence_limit(mut self, limit: Duration) -> Self {
        self.silence = Some(limit);
        self
    }

    /// The next line of stdout, or why there is none.
    ///
    /// `stop` is typically `cancellation.cancelled()`; whichever of the line, `stop`, the
    /// deadline and the silence limit comes first decides, and all but the line kill the process
    /// before answering. Errors when stdout was [`Stdout::Discarded`].
    pub async fn next_line(&mut self, stop: impl Future<Output = ()>) -> Result<ToolLine> {
        if self.lines.is_none() {
            let stdout = self
                .stdout
                .take()
                .context("the tool's stdout is not read")?;
            self.lines = Some(BufReader::new(stdout));
        }
        let Some(lines) = self.lines.as_mut() else {
            return Ok(ToolLine::End);
        };
        // The silence clock starts with every wait for a line, so each line resets it.
        let quiet_until = self.silence.map(|limit| Instant::now() + limit);
        let deadline = match (self.deadline, quiet_until) {
            (Some(deadline), Some(quiet_until)) => Some(deadline.min(quiet_until)),
            (deadline, quiet_until) => deadline.or(quiet_until),
        };
        tokio::select! {
            () = stop => {
                let _ = self.child.kill().await;
                Ok(ToolLine::Stopped)
            }
            () = expiry(deadline) => {
                let _ = self.child.kill().await;
                Ok(ToolLine::TimedOut)
            }
            line = read_line_lossy(lines) => Ok(line?.map_or(ToolLine::End, ToolLine::Line)),
        }
    }

    /// Waits for the process to end, unless `stop` fires or the deadline passes first; both
    /// kill it.
    pub async fn wait_or_stop(&mut self, stop: impl Future<Output = ()>) -> Result<ToolEnd> {
        let deadline = self.deadline;
        tokio::select! {
            () = stop => {
                let _ = self.child.kill().await;
                Ok(ToolEnd::Stopped)
            }
            () = expiry(deadline) => {
                let _ = self.child.kill().await;
                Ok(ToolEnd::TimedOut)
            }
            status = self.child.wait() => Ok(ToolEnd::Exited(status?)),
        }
    }

    /// Waits for the process to end.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Kills the process, best effort.
    ///
    /// The result is dropped on purpose: the only reasons to call this are a cancellation and a
    /// deliberate split, and in both the caller has already decided the outcome. A kill that
    /// fails because the process had just exited by itself must not turn either into an error.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// The end of what the process wrote to stderr, after it has ended.
    ///
    /// Empty when the drain task itself was cancelled or panicked — a lost diagnostic must not
    /// change how the run is reported, which is why this returns a `String` and not a `Result`.
    pub async fn stderr(self) -> String {
        self.stderr.await.unwrap_or_default()
    }
}

/// Asks a Python tool to write UTF-8 to its pipes.
///
/// yt-dlp, gallery-dl and streamlink are Python programs, and Python writes to a pipe in the
/// locale's encoding — on Windows a code page such as cp1252. A title with an emoji then reached
/// the runner as bytes that are not UTF-8, and the strict line reader failed the whole download
/// with "stream did not contain valid UTF-8" (owner report, 2026-10-06, a Facebook reel). With
/// UTF-8 the file name in yt-dlp's final-path line also survives; ffmpeg and the other
/// non-Python tools ignore both variables.
fn speak_utf8(command: &mut Command) {
    command
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
}

/// The next line of `reader` without its line ending, or `None` at the end of the stream.
///
/// Lossy on purpose: one byte a tool wrote in another encoding must cost at most a replaced
/// character in a log line, never the download. Not cancel-safe, which does not matter here —
/// when another branch of [`ToolProcess::next_line`]'s select wins, the process is killed and
/// its stdout is not read again.
async fn read_line_lossy(reader: &mut BufReader<ChildStdout>) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    if reader.read_until(b'\n', &mut bytes).await? == 0 {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Resolves at `deadline`, or never without one.
async fn expiry(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Runs a short external invocation to completion under a timeout and collects its output.
///
/// The counterpart to [`ToolProcess`] for the *other* shape of external-tool run: a question
/// rather than a download. `rd-media` asks `yt-dlp -J` for a page's metadata, `rd-stream` asks
/// `streamlink --json` whether a channel is live, and [`crate::version`] asks every managed tool
/// what version it is. Each rebuilt the same four lines around `.output()`, each with its own
/// console-window flag — the one setting whose absence is invisible on the machine that wrote
/// it and pops a console window on every Windows install.
///
/// **The nested result is deliberate.** It is exactly what `tokio::time::timeout` returns, and
/// it is handed back rather than collapsed because the three callers answer a failed run in
/// three different vocabularies — a stable-coded [`Failure`], an `anyhow` context, a silent
/// default — and *which* of the two failures happened decides the answer. An error type of this
/// module's own would force one vocabulary on all three and move those messages away from the
/// code that owns them; this costs one `?` at each call site and moves nothing.
///
/// stdin is the null device, because a tool that decides to prompt must fail rather than wait
/// for a keystroke nobody will type, and `kill_on_drop` is on so a run abandoned at the timeout
/// does not leave a process behind still doing the work nobody is waiting for any more. The
/// environment and the working directory (`crate::workdir`) are those of [`ToolProcess::spawn`].
///
/// **There is no stdout/stderr argument, and that is not an omission.**
/// `tokio::process::Command::output` configures both as pipes *unconditionally*, overriding
/// whatever the caller set. `rd-stream`'s probe asked for `Stdio::null()` on stderr and had it
/// captured anyway for as long as it has existed: the line read as a guarantee and was not one,
/// and offering the choice here would reproduce that lie behind a nicer name. A caller that
/// genuinely must discard a stream has to spawn the process itself — that is [`ToolProcess`].
pub async fn run_to_output(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::io::Result<Output>, Elapsed> {
    rd_files::restrict_environment(command, rd_files::TOOL_VARIABLES);
    speak_utf8(command);
    crate::workdir::isolate(command);
    command
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .no_console_window();
    tokio::time::timeout(timeout, command.output()).await
}

/// Rate limit for progress writes.
///
/// [`Self::due`] and [`Self::mark`] are separate so the interval is measured from the moment
/// the previous write *finished*. Restarting it before the await would make a slow writer
/// produce writes faster, which is the opposite of what a throttle is for.
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last: tokio::time::Instant,
}

impl ProgressThrottle {
    /// A throttle that is due immediately, so the first line a tool prints is reported at once
    /// instead of after a silent first interval.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: tokio::time::Instant::now() - interval,
        }
    }

    /// Whether another write is allowed yet.
    #[must_use]
    pub fn due(&self) -> bool {
        self.last.elapsed() >= self.interval
    }

    /// Records that a write has just completed.
    pub fn mark(&mut self) {
        self.last = tokio::time::Instant::now();
    }
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new(PROGRESS_INTERVAL)
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
