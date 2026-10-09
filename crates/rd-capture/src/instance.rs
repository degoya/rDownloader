//! One agent per account (RD-1200-03, `docs/security/capture-agent.md` finding 9).
//!
//! The Click'n'Load bind used to be the only thing that kept a second agent out, and it does not
//! when the bind is partial: with `127.0.0.1:9666` taken and `[::1]:9666` free, two agents ran and
//! watched the clipboard twice. An exclusive lock on a file in the configuration directory holds
//! for the life of the process and is released by the system however the process ends.
//!
//! The lock waits: on Windows the relaunch after an update starts the new agent before the old
//! one has ended (`relaunch.rs`), so a lock that failed at once would end the new agent instead
//! of the old one. A second agent started by hand waits the same few seconds and then says that
//! one is running already.

use std::{
    fs::{File, OpenOptions, TryLockError},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

/// How long a starting agent waits for the one it replaces.
pub(crate) const RELAUNCH_WAIT: Duration = Duration::from_secs(10);

/// How often the lock is tried while it is held.
const RETRY_EVERY: Duration = Duration::from_millis(100);

/// The lock file, in the agent's configuration directory.
const LOCK_FILE: &str = "agent.lock";

/// Held for as long as the agent runs.
pub(crate) struct InstanceLock {
    _file: File,
}

/// Another agent of this account holds the lock and did not end within the wait.
#[derive(Debug)]
pub(crate) struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("another rdownloader-capture of this account is running")
    }
}

impl std::error::Error for AlreadyRunning {}

/// Takes the lock in `directory`, waiting at most `wait` for an agent that holds it to end.
pub(crate) fn acquire(directory: &Path, wait: Duration) -> Result<InstanceLock> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let path = directory.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(InstanceLock { _file: file }),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(RETRY_EVERY);
            }
            Err(TryLockError::WouldBlock) => return Err(AlreadyRunning.into()),
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("lock {}", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{AlreadyRunning, acquire};

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("rd-capture-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn a_second_agent_is_refused_once_the_wait_is_over() {
        let directory = scratch("second");
        let _first = acquire(&directory, Duration::ZERO).expect("the first agent takes the lock");
        let started = Instant::now();
        let refused = acquire(&directory, Duration::from_millis(300))
            .err()
            .expect("the second agent does not run beside the first");
        assert!(refused.is::<AlreadyRunning>(), "{refused:?}");
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "it waited first"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A launcher tells "already running" from a crash by the code alone.
    #[test]
    fn a_running_agent_ends_the_start_with_the_already_running_code() {
        let error = anyhow::Error::new(AlreadyRunning).context("start the capture agent");
        assert_eq!(crate::report(&error), crate::config::EXIT_PORT_BUSY);
    }

    /// The relaunch on Windows: the new agent starts while the old one is still ending, and
    /// runs once it has.
    #[test]
    fn the_relaunched_agent_waits_for_the_old_one_to_end() {
        let directory = scratch("relaunch");
        let old = acquire(&directory, Duration::ZERO).expect("the old agent holds the lock");
        let ending = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            drop(old);
        });
        let new = acquire(&directory, Duration::from_secs(10));
        ending.join().expect("the old agent ended");
        assert!(new.is_ok(), "{:?}", new.err());
        drop(new);
        // And the lock is free again once the agent holding it is gone.
        assert!(acquire(&directory, Duration::ZERO).is_ok());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
