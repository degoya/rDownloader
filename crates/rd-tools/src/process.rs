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
//! What is *not* here: the loop. Each runner reads a different thing out of its process —
//! yt-dlp's progress lines, gallery-dl's stored paths, streamlink's growing file — and folding
//! those into one callback would hide the differences rather than remove them.
//!
//! There are two shapes, not one. [`ToolProcess`] is the long-running download that is followed
//! until it ends; [`run_to_output`] is the short question — `yt-dlp -J`, `streamlink --json`,
//! `<tool> --version` — that is asked under a timeout and answered in seconds. They share the
//! stdin, `kill_on_drop` and console-window wiring and nothing else, so they are two functions
//! over one set of rules rather than one function with a mode flag.

use std::{path::PathBuf, process::Output, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use rd_core::{Failure, FailureKind, ToolLease};
use tokio::{
    process::{Child, ChildStdout, Command},
    time::error::Elapsed,
};

/// How often a running external tool's progress is written to the queue row.
///
/// Every write is a serialized-writer round trip, and a tool that reports ten lines a second
/// would otherwise turn one download into a write storm.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(750);

/// `CREATE_NO_WINDOW`. Without it every external tool flashes a console window on Windows,
/// including for a service started at login with no desktop session to show it in.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    stderr: tokio::task::JoinHandle<String>,
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
    pub fn spawn(command: &mut Command, name: &str, stdout: Stdout) -> Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(match stdout {
                Stdout::Read => Stdio::piped(),
                Stdout::Discarded => Stdio::null(),
            })
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command.spawn().with_context(|| format!("spawn {name}"))?;
        let stdout = child.stdout.take();
        let mut handle = child
            .stderr
            .take()
            .with_context(|| format!("{name} stderr"))?;
        let stderr = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut handle, &mut buffer).await;
            String::from_utf8_lossy(&buffer).into_owned()
        });
        Ok(Self {
            child,
            stdout,
            stderr,
        })
    }

    /// The child's stdout, once. `None` for [`Stdout::Discarded`], and on the second call.
    ///
    /// The caller adds its own context, because "yt-dlp stdout" is the message its tests and
    /// its logs already carry.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
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

    /// Everything the process wrote to stderr, after it has ended.
    ///
    /// Empty when the drain task itself was cancelled or panicked — a lost diagnostic must not
    /// change how the run is reported, which is why this returns a `String` and not a `Result`.
    pub async fn stderr(self) -> String {
        self.stderr.await.unwrap_or_default()
    }
}

/// Runs a short external invocation to completion under a timeout and collects its output.
///
/// The counterpart to [`ToolProcess`] for the *other* shape of external-tool run: a question
/// rather than a download. `rd-media` asks `yt-dlp -J` for a page's metadata, `rd-stream` asks
/// `streamlink --json` whether a channel is live, and [`crate::version`] asks every managed tool
/// what version it is. Each rebuilt the same four lines around `.output()`, each with its own
/// literal `0x0800_0000` — the one constant whose absence is invisible on the machine that wrote
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
/// does not leave a process behind still doing the work nobody is waiting for any more.
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
    command.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
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
mod tests {
    use std::time::Duration;

    use super::{PROGRESS_INTERVAL, ProgressThrottle, prepare, run_to_output};

    #[tokio::test]
    async fn a_tool_that_was_not_found_is_unsupported_and_names_itself() {
        let failure = prepare("yt-dlp", None, crate::Capability::MediaDownload)
            .await
            .expect_err("missing tool fails");
        // The three runners reported exactly this before the scaffolding was lifted; the
        // code and the parameter are what the web client translates and offers to install.
        assert_eq!(failure.code.as_deref(), Some("media.tool_missing"));
        assert!(matches!(
            failure.category,
            rd_core::FailureKind::Unsupported
        ));
        assert_eq!(
            failure.params.get("tool").map(String::as_str),
            Some("yt-dlp")
        );
        assert_eq!(failure.message, "yt-dlp is not installed or not configured");
    }

    #[tokio::test]
    async fn a_binary_that_does_not_exist_is_a_spawn_failure_and_not_a_timeout() {
        let mut command = tokio::process::Command::new("rd-tools-no-such-binary-exists");
        let result = run_to_output(&mut command, Duration::from_secs(30)).await;
        // The two failures are handed back separately on purpose. rd-media reports a timeout
        // as `media.probe_timeout` and a failed spawn as `media.tool_error`, and rd-stream
        // gives them different contexts; collapsing them here would take that choice away.
        // A missing binary must also not sit out the full timeout before it is noticed.
        assert!(matches!(result, Ok(Err(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn both_streams_are_captured_whatever_the_caller_configured() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "printf out; printf err >&2"]);
        // Set here deliberately, and ignored: `Command::output` pipes stdout and stderr
        // unconditionally. rd-stream's probe carried exactly this line and had stderr
        // captured for as long as it existed, which is why `run_to_output` offers no stdio
        // argument — it cannot honour one. If tokio ever stops overriding it, this fails.
        command.stderr(std::process::Stdio::null());
        let output = run_to_output(&mut command, Duration::from_secs(30))
            .await
            .expect("the tool finished inside the timeout")
            .expect("/bin/sh spawns");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_that_never_answers_is_a_timeout() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let result = run_to_output(&mut command, Duration::from_millis(50)).await;
        // `kill_on_drop` is what keeps this from leaving a `sleep` behind: the timeout drops
        // the future that owns the child, and the child goes with it.
        assert!(result.is_err());
    }

    #[test]
    fn a_fresh_throttle_is_due_and_stays_marked() {
        let mut throttle = ProgressThrottle::new(PROGRESS_INTERVAL);
        assert!(throttle.due());
        throttle.mark();
        assert!(!throttle.due());
    }
}
