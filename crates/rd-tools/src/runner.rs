//! What the queue runners around an external tool share beyond the process itself (audit 1.9.1,
//! TR-07, TR-13): how many files they run at once, and how a failed run is reported.
//!
//! `rd-media`, `rd-gallery` and `rd-stream` each clamped their parallel-file setting the same
//! way — once, when the runner was built, so a changed setting waited for a restart while FTP,
//! SFTP and buckets read theirs live — and `rd-gallery`, `rd-stream` and `rd-extract` each cut
//! the same last line out of stderr for the same retryable failure.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rd_core::{Failure, FailureKind};
use tokio::sync::RwLock;

/// The most files one tool runner works on at once.
pub const MAX_PARALLEL_FILES: u32 = 8;

/// What a runner assumes until it has read its settings once.
pub const DEFAULT_PARALLEL_FILES: usize = 2;

/// How much of a tool's last stderr line a failure quotes.
pub const STDERR_TAIL_CHARS: usize = 300;

/// The parallel-file setting as a slot count: at least one, at most [`MAX_PARALLEL_FILES`].
#[must_use]
pub fn parallel_files(configured: u32) -> usize {
    configured.clamp(1, MAX_PARALLEL_FILES) as usize
}

/// A runner's slot count, read from its live settings on every dispatch pass.
///
/// `slot_capacity` is synchronous and the settings sit behind an async lock, so the read is a
/// `try_read`; while a writer holds the lock the value read last stands in, never a constant.
pub struct LiveSlots<T> {
    settings: Arc<RwLock<T>>,
    read: fn(&T) -> u32,
    last: AtomicUsize,
}

impl<T> LiveSlots<T> {
    /// Slots out of `settings`, the field picked by `read`.
    #[must_use]
    pub fn new(settings: Arc<RwLock<T>>, read: fn(&T) -> u32) -> Self {
        let slots = Self {
            settings,
            read,
            last: AtomicUsize::new(DEFAULT_PARALLEL_FILES),
        };
        slots.get();
        slots
    }

    /// The slot count the settings say right now.
    pub fn get(&self) -> usize {
        match self.settings.try_read() {
            Ok(settings) => {
                let slots = parallel_files((self.read)(&settings));
                self.last.store(slots, Ordering::Release);
                slots
            }
            Err(_) => self.last.load(Ordering::Acquire),
        }
    }
}

/// The last non-empty line of a tool's stderr, redacted and cut to [`STDERR_TAIL_CHARS`];
/// `fallback` when it printed nothing.
///
/// Redacted because it becomes a failure message on the download row and in the interface,
/// and a tool readily echoes the signed address it was just refused.
#[must_use]
pub fn stderr_tail(stderr: &str, fallback: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(fallback);
    rd_core::redact_text(line)
        .chars()
        .take(STDERR_TAIL_CHARS)
        .collect()
}

/// A tool run that failed for no reason the runner recognised: retried in two minutes, with
/// the stderr tail as its message and its `message` parameter.
#[must_use]
pub fn tool_failed(code: &str, stderr: &str, fallback: &str) -> Failure {
    let tail = stderr_tail(stderr, fallback);
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: Some(120),
        },
        code,
        tail.clone(),
    )
    .with_param("message", tail)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use super::{LiveSlots, parallel_files, stderr_tail, tool_failed};

    #[test]
    fn the_setting_is_clamped_to_a_usable_count() {
        assert_eq!(parallel_files(0), 1);
        assert_eq!(parallel_files(3), 3);
        assert_eq!(parallel_files(64), 8);
    }

    /// TR-07: a changed setting reaches the next dispatch pass, not the next start.
    #[tokio::test]
    async fn the_slot_count_follows_the_settings_live() {
        let settings = Arc::new(RwLock::new(3_u32));
        let slots = LiveSlots::new(Arc::clone(&settings), |value| *value);
        assert_eq!(slots.get(), 3);
        *settings.write().await = 5;
        assert_eq!(slots.get(), 5);
        // A writer holding the lock leaves the last value read, not a default.
        let held = settings.write().await;
        assert_eq!(slots.get(), 5);
        drop(held);
    }

    #[test]
    fn the_tail_is_the_last_line_said() {
        assert_eq!(
            stderr_tail("[info] one\nerror: HTTP 503\n\n", "tool failed"),
            "error: HTTP 503"
        );
        assert_eq!(stderr_tail("  \n", "tool failed"), "tool failed");
        assert_eq!(stderr_tail(&"x".repeat(1000), "-").len(), 300);
    }

    #[test]
    fn an_unrecognised_failure_is_retried_with_its_tail() {
        let failure = tool_failed(
            "gallery.tool_failed",
            "error: HTTPError 503",
            "gallery-dl failed",
        );
        assert_eq!(failure.code.as_deref(), Some("gallery.tool_failed"));
        assert!(matches!(
            failure.category,
            rd_core::FailureKind::Transient {
                retry_after_seconds: Some(120)
            }
        ));
        assert_eq!(
            failure.params.get("message").map(String::as_str),
            Some("error: HTTPError 503")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lines_are_read_until_the_tool_is_done() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "echo one; echo two"]);
        let mut process =
            crate::ToolProcess::spawn(&mut command, "lines", crate::Stdout::Read).expect("spawn");
        let mut seen = Vec::new();
        loop {
            match process
                .next_line(std::future::pending())
                .await
                .expect("read")
            {
                crate::ToolLine::Line(line) => seen.push(line),
                crate::ToolLine::End => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(seen, ["one", "two"]);
        assert!(process.wait().await.expect("wait").success());
    }

    /// TR-13 / INTAKE-05: a tool that hangs is killed at its limit instead of holding its job.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hanging_tool_is_killed_at_its_deadline() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let mut process = crate::ToolProcess::spawn(&mut command, "hang", crate::Stdout::Read)
            .expect("spawn")
            .with_deadline(std::time::Duration::from_millis(50));
        let answer = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            process.next_line(std::future::pending()),
        )
        .await
        .expect("the deadline ended the wait")
        .expect("read");
        assert_eq!(answer, crate::ToolLine::TimedOut);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stop_kills_the_tool() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let mut process = crate::ToolProcess::spawn(&mut command, "stop", crate::Stdout::Discarded)
            .expect("spawn");
        let end = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            process.wait_or_stop(std::future::ready(())),
        )
        .await
        .expect("the stop ended the wait")
        .expect("wait");
        assert!(matches!(end, crate::ToolEnd::Stopped));
    }
}
