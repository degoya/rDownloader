//! The processes of an update (RD-180-02): the updater the service starts, the service the
//! updater starts, and the Windows installer.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

use super::{APPLY_COMMAND, Journal, remove_any, update_dir};

/// Inside `<data>/update`: the copy of the executable the updater runs from.
pub const UPDATER_DIR: &str = "updater";
/// Inside `<data>/update`: what the updater writes to its output.
pub const UPDATER_LOG: &str = "updater.log";

/// Windows Installer's exit codes that mean the package is installed: done, and done with a
/// restart that is needed (3010) or was started (1641).
pub const MSI_SUCCESS: [i32; 3] = [0, 3010, 1641];

/// Starts `program` in `cwd`, detached from this process — its own process group on Unix, no
/// console and a group of its own on Windows, outside the caller's job where Windows allows it —
/// so it outlives the process that started it and a Ctrl-C meant for that one does not reach it.
/// Standard output and error are appended to the two files.
///
/// # Errors
///
/// When a log cannot be opened or the program cannot be started.
pub fn spawn_detached<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    cwd: &Path,
    stdout: &Path,
    stderr: &Path,
) -> Result<Child> {
    let open = |path: &Path| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(open(stdout)?)
        .stderr(open(stderr)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
        match command.spawn() {
            Ok(child) => return Ok(child),
            // A job that allows no breakaway: started inside it, as any other child would be.
            Err(error) if error.raw_os_error() == Some(5) => {
                command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("start {}", program.display()));
            }
        }
    }
    command
        .spawn()
        .with_context(|| format!("start {}", program.display()))
}

/// The updater's file name inside [`UPDATER_DIR`]: not `rdownloader(.exe)`, so nothing that stops
/// the service by its image name (the MSI's upgrade, `stop-rdownloader.bat`) stops the updater.
#[must_use]
pub fn updater_file_name() -> &'static str {
    if cfg!(windows) {
        "rdownloader-updater.exe"
    } else {
        "rdownloader-updater"
    }
}

/// Copies the running executable to `<data>/update/updater/` and starts the copy as
/// `apply-update --journal <journal>`: a program cannot replace its own running file on Windows,
/// and outside the program folder the copy is never part of what the switch moves.
///
/// # Errors
///
/// When the copy or the start fails; the journal then still says `handed`, and the service ends
/// it as failed.
pub fn launch_updater(journal: &Journal) -> Result<()> {
    let current = std::env::current_exe().context("locate the running executable")?;
    let update = update_dir(&journal.plan.data_dir);
    let directory = update.join(UPDATER_DIR);
    remove_any(&directory)?;
    fs::create_dir_all(&directory).with_context(|| format!("create {}", directory.display()))?;
    let copy = directory.join(updater_file_name());
    fs::copy(&current, &copy)
        .with_context(|| format!("copy {} to {}", current.display(), copy.display()))?;
    let log = update.join(UPDATER_LOG);
    let journal_path = Journal::path(&journal.plan.data_dir);
    let args = [
        OsStr::new(APPLY_COMMAND),
        OsStr::new("--journal"),
        journal_path.as_os_str(),
    ];
    spawn_detached(&copy, &args[..], &journal.plan.service_cwd, &log, &log)?;
    Ok(())
}

/// Starts `executable` as the service was started — its arguments, its folder — with its output
/// appended to `logs/rdownloader.log` and `logs/rdownloader.err.log` there, as the launchers and
/// the autostart write them, and its process id in `run/rdownloader.pid` where a launcher keeps
/// one, so `stop-rdownloader` finds it.
///
/// # Errors
///
/// When the log folder cannot be created or the program cannot be started.
pub fn start_service(journal: &Journal, executable: &Path) -> Result<Child> {
    let cwd = &journal.plan.service_cwd;
    let logs = cwd.join("logs");
    fs::create_dir_all(&logs).with_context(|| format!("create {}", logs.display()))?;
    let child = spawn_detached(
        executable,
        journal.plan.service_args.as_slice(),
        cwd,
        &logs.join("rdownloader.log"),
        &logs.join("rdownloader.err.log"),
    )?;
    let run = cwd.join("run");
    if run.is_dir()
        && let Err(error) = fs::write(run.join("rdownloader.pid"), format!("{}\n", child.id()))
    {
        tracing::warn!(%error, "the service's process id file could not be written");
    }
    Ok(child)
}

/// The arguments of one `msiexec` run: `/i` installs, `/x` removes, both silent, without a
/// restart, with a verbose log beside the journal; `properties` are `NAME=value` pairs.
#[must_use]
pub fn msiexec_arguments(
    action: &str,
    package: &Path,
    log: &Path,
    properties: &[&str],
) -> Vec<String> {
    let mut args = vec![
        action.to_owned(),
        package.display().to_string(),
        "/qn".to_owned(),
        "/norestart".to_owned(),
        "/l*v".to_owned(),
        log.display().to_string(),
    ];
    args.extend(properties.iter().map(|property| (*property).to_owned()));
    args
}

/// `<SystemRoot>\System32\msiexec.exe`, from `SystemRoot` or `C:\Windows` without it.
///
/// Never the bare name (security review 2026-09-30, finding 2): Windows looks for it in the
/// folder of the running executable first, and a portable installation's folder may be
/// writable by every signed-in user, who would have planted the program an update runs.
#[must_use]
pub fn msiexec_path() -> PathBuf {
    system_program(std::env::var_os("SystemRoot"), "msiexec.exe")
}

/// `name` in the `System32` folder of the Windows directory `root`, `C:\Windows` when unknown.
#[must_use]
pub fn system_program(root: Option<std::ffi::OsString>, name: &str) -> PathBuf {
    let root = root
        .filter(|root| !root.is_empty())
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(name)
}

/// Runs `msiexec` to its end and returns its exit code.
///
/// # Errors
///
/// When `msiexec` cannot be started or ends without a code.
pub fn run_msiexec(args: &[String]) -> Result<i32> {
    let status = Command::new(msiexec_path())
        .args(args)
        .stdin(Stdio::null())
        .status()
        .context("start msiexec")?;
    status.code().context("msiexec ended without an exit code")
}

/// Where `msiexec` writes its log for `step`.
#[must_use]
pub fn msiexec_log(journal: &Journal, step: &str) -> PathBuf {
    update_dir(&journal.plan.data_dir).join(format!("msiexec-{step}.log"))
}
