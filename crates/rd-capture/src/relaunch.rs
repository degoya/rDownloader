//! The agent after a self-update (RD-190-07, its core brought forward to 1.8.1): the updater
//! replaces the program files but never the running agent, which lives in the person's session
//! and not as a child of the service. On Windows the old agent then kept running from
//! `.previous\` and kept that folder from being removed (live finding 2026-10-02).
//!
//! So the agent watches its own program file, and once another file stands in its place it ends
//! and starts that one with the arguments it was started with. The agent decides, not the
//! service: the service knows neither the agent's session nor its process, an agent may serve a
//! service on another machine, and a file replaced under the agent is a fact it reads without a
//! token, an endpoint or a version handshake — after the portable switch, the MSI, a package
//! manager's upgrade and a rollback alike.
//!
//! **When**: two looks in a row, [`CHECK_INTERVAL`] apart, see the same other file. A switch in
//! progress (the file briefly absent) or a file still being written never counts, and the agent
//! runs the new version within half a minute of the switch.
//!
//! **How**: Unix replaces the process image (`exec`), so a systemd unit or a launchd job goes on
//! tracking the same process; Windows starts the new program without a console window and the
//! old one ends. Either happens after the Click'n'Load listeners are closed, so the new agent gets
//! the port.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

/// How often the agent looks at its program file.
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(15);

/// What tells one program file from another without reading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    /// Device and inode: a file renamed over this one is another inode, whatever its times say.
    #[cfg(unix)]
    file: (u64, u64),
}

impl Fingerprint {
    /// The file at `path` as it is now; `None` while there is none.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            file: {
                use std::os::unix::fs::MetadataExt as _;
                (metadata.dev(), metadata.ino())
            },
        })
    }
}

/// The rule over successive looks at the program file. Pure, so it is tested on every host.
#[derive(Debug)]
pub(crate) struct ReplacementWatch {
    started: Fingerprint,
    /// The other file the last look saw.
    seen: Option<Fingerprint>,
}

impl ReplacementWatch {
    pub(crate) fn new(started: Fingerprint) -> Self {
        Self {
            started,
            seen: None,
        }
    }

    /// One look; `true` once the same other file was seen twice in a row.
    pub(crate) fn observe(&mut self, now: Option<Fingerprint>) -> bool {
        let Some(now) = now.filter(|now| *now != self.started) else {
            // Absent (a switch between two renames) or back as it was (taken back in time).
            self.seen = None;
            return false;
        };
        if self.seen.as_ref() == Some(&now) {
            return true;
        }
        self.seen = Some(now);
        false
    }
}

/// Marker in the error chain: the agent ended because its program file was replaced, and the
/// process is to continue as the new program.
#[derive(Debug)]
pub(crate) struct Replaced {
    pub(crate) program: PathBuf,
}

impl std::fmt::Display for Replaced {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} was replaced", self.program.display())
    }
}

impl std::error::Error for Replaced {}

/// Looks at `program` every [`CHECK_INTERVAL`] until it was replaced — then cancels the agent and
/// returns `true` — or until the agent ends otherwise.
pub(crate) async fn watch(program: PathBuf, cancellation: CancellationToken) -> bool {
    let Some(started) = Fingerprint::of(&program) else {
        tracing::warn!(
            program = %program.display(),
            "the agent's program file cannot be read; it does not restart after an update"
        );
        return false;
    };
    let mut watch = ReplacementWatch::new(started);
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return false,
            () = tokio::time::sleep(CHECK_INTERVAL) => {}
        }
        if watch.observe(Fingerprint::of(&program)) {
            tracing::info!(
                program = %program.display(),
                "the agent's program file was replaced; the agent restarts as the new one"
            );
            cancellation.cancel();
            return true;
        }
    }
}

/// Continues as the program that replaced this one, with this process's arguments, and returns
/// the exit code for this process: 0 once the new one runs (Windows), 1 when it could not be
/// started. On Unix it returns only on failure, because the new program takes over the process.
pub(crate) fn relaunch(replaced: &Replaced) -> u8 {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut command = std::process::Command::new(&replaced.program);
    command.args(&args).stdin(std::process::Stdio::null());
    #[cfg(unix)]
    let error = {
        use std::os::unix::process::CommandExt as _;
        command.exec()
    };
    #[cfg(not(unix))]
    let error = {
        rd_files::NoConsoleWindow::no_console_window(&mut command);
        match command.spawn() {
            Ok(_) => return 0,
            Err(error) => error,
        }
    };
    tracing::error!(
        %error,
        program = %replaced.program.display(),
        "the replaced agent could not be started; start it again by hand"
    );
    1
}

#[cfg(test)]
mod tests {
    use super::{Fingerprint, ReplacementWatch};

    /// A fresh directory of this test's own.
    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("rd-capture-relaunch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create the scratch directory");
        directory
    }

    fn file(directory: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, text).expect("write");
        path
    }

    #[test]
    fn a_program_renamed_over_the_running_one_is_another_file() {
        let directory = scratch("renamed");
        let program = file(&directory, "rdownloader-capture", "1.8.0");
        let started = Fingerprint::of(&program).expect("there");
        assert_eq!(Fingerprint::of(&program), Some(started.clone()));
        // What the switch does: the old file aside, the new one renamed into its place.
        let incoming = file(&directory, "incoming", "1.8.1 and longer");
        std::fs::rename(&program, directory.join("aside")).expect("aside");
        assert_eq!(Fingerprint::of(&program), None, "between the two renames");
        std::fs::rename(&incoming, &program).expect("place");
        let replaced = Fingerprint::of(&program).expect("there again");
        assert_ne!(replaced, started);
    }

    #[test]
    fn the_agent_restarts_once_the_other_file_stays() {
        let directory = scratch("stays");
        let old = Fingerprint::of(&file(&directory, "old", "1.8.0")).expect("old");
        let new = Fingerprint::of(&file(&directory, "new", "1.8.1 and longer")).expect("new");
        let mut watch = ReplacementWatch::new(old.clone());
        assert!(!watch.observe(Some(old)), "nothing changed");
        assert!(!watch.observe(None), "the switch is between two renames");
        assert!(!watch.observe(Some(new.clone())), "seen once");
        assert!(watch.observe(Some(new)), "seen twice in a row");
    }

    #[test]
    fn a_file_that_is_gone_again_or_back_as_it_was_does_not_restart_the_agent() {
        let directory = scratch("gone");
        let old = Fingerprint::of(&file(&directory, "old", "1.8.0")).expect("old");
        let new = Fingerprint::of(&file(&directory, "new", "1.8.1 and longer")).expect("new");
        let other =
            Fingerprint::of(&file(&directory, "other", "still being written")).expect("other");
        let mut watch = ReplacementWatch::new(old.clone());
        assert!(!watch.observe(Some(new.clone())));
        assert!(!watch.observe(None), "gone between two looks");
        assert!(
            !watch.observe(Some(new.clone())),
            "counts from the start again"
        );
        assert!(
            !watch.observe(Some(old)),
            "taken back before the second look"
        );
        assert!(!watch.observe(Some(new)));
        assert!(!watch.observe(Some(other)), "a file that still changes");
    }
}
