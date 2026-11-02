//! What a program started by its autostart entry does for itself (RD-1400-07).
//!
//! On Windows the `Run` entry used to start `wscript.exe` with a VBScript that ran a hidden
//! `.cmd`, and the `.cmd` moved into the working folder and sent the program's output into the
//! log files. Microsoft Defender's heuristic took that chain for malware persistence
//! (`Trojan:Win32/Bearfoos.A!ml`). The `Run` value now starts the capture agent directly, and the
//! agent does the `.cmd`'s work in safe Rust: [`enter_autostart`] when it starts for itself,
//! [`start_beside`] when it starts the service for the service's own entry.
//!
//! Compiled and tested on every platform; only the Windows `Run` value asks for it today.

use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use rd_files::NoConsoleWindow as _;

use super::{Registration, Target, stable_executable_path};

/// The flag `rdownloader-capture run` is started with by its Windows `Run` entry: move into the
/// registration's working folder and write the logs there ([`enter_autostart`]).
pub const AUTOSTART_FLAG: &str = "--autostart";

/// The capture agent's command the service's Windows `Run` entry starts: it starts the service
/// beside it without a window ([`start_beside`]) and ends.
pub const START_SERVICE_COMMAND: &str = "start-service";

/// The two log files of an autostarted program, opened for appending.
#[derive(Debug)]
pub struct AutostartLogs {
    /// `logs/<program>.log`: what used to be the program's redirected standard output.
    pub output: File,
    /// `logs/<program>.err.log`: what used to be its redirected standard error.
    pub errors: File,
}

impl AutostartLogs {
    fn open(registration: &Registration) -> Result<Self> {
        Ok(Self {
            output: append(&registration.stdout_path())?,
            errors: append(&registration.stderr_path())?,
        })
    }
}

/// What `target` does first when its autostart entry started it: moves into the folder its
/// registration names (the data folder of an installed build, otherwise the program's own) and
/// opens its log files there, which the program then writes instead of a console.
///
/// # Errors
///
/// When the running program cannot be located, the folder cannot be entered or a log file
/// cannot be opened.
pub fn enter_autostart(target: Target) -> Result<AutostartLogs> {
    let executable = std::env::current_exe().context("locate the running program")?;
    let registration = Registration::new(target, &stable_executable_path(&executable))?;
    std::env::set_current_dir(&registration.working_directory).with_context(|| {
        format!(
            "enter the working folder {}",
            registration.working_directory.display()
        )
    })?;
    AutostartLogs::open(&registration)
}

/// Starts `target`'s program from the folder of `sibling` in the background: its autostart
/// argument, its registration's working folder, its output appended to its log files, no console
/// window on Windows. Returns once it is started; it is not waited for.
///
/// The program gets none of the caller's standard handles: its input is the null device, its
/// output goes to its log files, and on Windows the caller's own standard handles are made
/// non-inheritable first — a Windows child receives every inheritable handle, and a caller's
/// pipe the service held open kept `rdownloader-capture start-service | Out-Host` waiting for as
/// long as the service ran (RD-1410-02). So the caller, which ends right after, keeps them.
///
/// # Errors
///
/// When the program is not beside `sibling` or cannot be started; a start that fails is also
/// written into the program's error log, where its own failures go.
pub fn start_beside(target: Target, sibling: &Path) -> Result<()> {
    let executable = sibling
        .parent()
        .context("the running program has no folder")?
        .join(executable_name(target));
    if !executable.is_file() {
        bail!(
            "{} was not found beside {}",
            executable_name(target).display(),
            sibling.display()
        );
    }
    start_detached(target, &executable)
}

fn start_detached(target: Target, executable: &Path) -> Result<()> {
    let registration = Registration::new(target, executable)?;
    let logs = AutostartLogs::open(&registration)?;
    let mut errors = logs.errors.try_clone().context("share the error log")?;
    // Best effort: a handle Windows will not change costs the caller's pipe, never the start.
    let _ = rd_files::keep_standard_handles_from_children();
    let started = Command::new(executable)
        .arg(target.argument())
        .current_dir(&registration.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(logs.output))
        .stderr(Stdio::from(logs.errors))
        .no_console_window()
        .spawn();
    match started {
        Ok(_child) => Ok(()),
        Err(error) => {
            use std::io::Write as _;
            let message = format!("could not start {}: {error}", executable.display());
            // Best effort: the log is the only place anybody looks for a missing autostart.
            let _ = writeln!(errors, "{message}");
            bail!(message)
        }
    }
}

/// The file name of `target`'s program on this platform.
fn executable_name(target: Target) -> PathBuf {
    PathBuf::from(format!("{}{}", target.slug(), std::env::consts::EXE_SUFFIX))
}

fn append(path: &Path) -> Result<File> {
    File::options()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt as _,
        path::Path,
        process::Stdio,
        time::{Duration, Instant},
    };

    use rd_files::NoConsoleWindow as _;

    use super::{Target, executable_name, start_beside};

    /// Set for the run of this test binary that plays the capture agent: the agent's path.
    const AGENT: &str = "RD_AUTOSTART_TEST_AGENT";
    const CALLER_TEST: &str = "launch::tests::the_callers_output_ends_while_the_service_runs";

    #[test]
    fn the_service_starts_beside_the_agent_with_its_folder_and_logs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let service = directory.path().join(executable_name(Target::Server));
        // Stands in for the service: says where it runs and with what, on both streams.
        std::fs::write(
            &service,
            "#!/bin/sh\necho \"out $1 $(pwd)\"\necho \"err $1\" >&2\n",
        )
        .expect("write the stand-in");
        std::fs::set_permissions(&service, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        let agent = directory.path().join(executable_name(Target::Capture));

        start_beside(Target::Server, &agent).expect("start the stand-in");

        let logs = directory.path().join("logs");
        let output = logs.join("rdownloader.log");
        let errors = logs.join("rdownloader.err.log");
        let mut written = String::new();
        for _ in 0..100 {
            written = std::fs::read_to_string(&errors).unwrap_or_default();
            if !written.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(written, "err serve\n");
        let output = std::fs::read_to_string(output).expect("read the output log");
        let folder = directory.path().canonicalize().expect("canonical folder");
        assert!(
            output.starts_with("out serve ") && output.contains(folder.to_str().expect("UTF-8")),
            "{output}"
        );
    }

    /// `rdownloader-capture start-service | Out-Host` (RD-1410-02): the caller reads the agent's
    /// output to its end, and that end comes when the agent ends, not when the service does.
    /// The test binary plays the agent, its output a pipe read to the end, while the stand-in
    /// service runs on for five seconds.
    #[test]
    fn the_callers_output_ends_while_the_service_runs() {
        if let Some(agent) = std::env::var_os(AGENT) {
            start_beside(Target::Server, Path::new(&agent)).expect("start the stand-in");
            return;
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let service = directory.path().join(executable_name(Target::Server));
        std::fs::write(&service, "#!/bin/sh\nsleep 5\n").expect("write the stand-in");
        std::fs::set_permissions(&service, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        let agent = directory.path().join(executable_name(Target::Capture));

        let started = Instant::now();
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", CALLER_TEST, "--nocapture"])
            .env(AGENT, &agent)
            .stdin(Stdio::null())
            .no_console_window()
            .output()
            .expect("run the agent");
        let elapsed = started.elapsed();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "the caller's output stayed open while the service ran: {elapsed:?}"
        );
    }

    #[test]
    fn a_missing_service_is_named() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let agent = directory.path().join(executable_name(Target::Capture));
        let error = start_beside(Target::Server, &agent).expect_err("nothing to start");
        assert!(
            format!("{error:#}").contains("rdownloader was not found beside"),
            "{error:#}"
        );
    }
}
