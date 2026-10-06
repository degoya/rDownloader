//! The self-update's process half (RD-180-02): `rdownloader apply-update`, and what `serve` does
//! with an update before it opens the database and once it answers.
//!
//! `apply-update` is hidden from the help: the service starts it from a copy of itself in
//! `<data>/update/updater/` (`rd_update::install::process::launch_updater`) with the journal it
//! wrote, and `.github/workflows/self-update.yml` runs it the same way. It holds the updater lock
//! for its whole life, then:
//!
//! 1. stops the service over the local control token (`rdownloader stop`'s way) and waits until
//!    it has saved its queue and ended; one that accepted the stop and is still running after
//!    [`STOP_WAIT`] is ended by force, by the process id and name the plan records;
//! 2. checks the artifact once more and the program folder (writable, twice the artifact free) —
//!    a refusal here changes nothing, and the old version is started again;
//! 3. switches: the portable archive through `rd_update::install::portable`, the Windows
//!    installer with `msiexec /i … /qn`, which Windows Installer undoes itself when it fails;
//! 4. starts the new version as the service was started and waits until its health route names
//!    the target version ([`HEALTH_TIMEOUT`] by default, the plan says);
//! 5. on a crash, a timeout or a wrong version: stops it, takes the switch back (the previous
//!    MSI reinstalled when one was kept), puts the database copy from before the update back,
//!    and starts the old version.
//!
//! Exit codes: `0` the new version runs, `2` the previous one runs again, `1` anything else; the
//! journal says what happened, and the restarted service shows it.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use rd_files::NoConsoleWindow as _;
use rd_update::InstallKind;
use rd_update::install::recover::{Recovery, confirm_started, recover_at_start};
use rd_update::install::{
    self, InstallError, Journal, Phase, UpdaterLock, portable, process, steps,
};

/// Honoured by a debug build only: the new version's health counts as failed once it answered,
/// so a test proves the way back without publishing a broken release.
const TEST_FAIL_HEALTH: &str = "RD_UPDATE_TEST_FAIL_HEALTH";
/// How long the service may take to save its queue and end.
const STOP_WAIT: Duration = Duration::from_secs(120);
/// The shortest wait for a health answer, whatever the plan says.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Args)]
pub(crate) struct ApplyArgs {
    /// The journal the service wrote: `<data>/update/journal.json`.
    #[arg(long)]
    journal: PathBuf,
}

/// A step's failure: its stable code and what happened.
type Failure = (&'static str, String);

pub(crate) async fn run(args: ApplyArgs) -> Result<()> {
    let data = args
        .journal
        .parent()
        .and_then(Path::parent)
        .context("the journal is not inside <data>/update")?
        .to_path_buf();
    let Some(lock) = UpdaterLock::acquire(&data)? else {
        anyhow::bail!("another updater runs for {}", data.display());
    };
    let mut journal =
        Journal::read(&data)?.context("no update is waiting in this data directory")?;
    anyhow::ensure!(
        journal.phase == Phase::Handed,
        "the update is {} already; only a handed-over one is applied",
        journal.phase.as_str()
    );
    apply(&mut journal).await?;
    tracing::info!(
        phase = journal.phase.as_str(),
        reason = journal.reason.as_deref().unwrap_or(""),
        "the updater ends"
    );
    drop(lock);
    std::process::exit(match journal.phase {
        Phase::Verified => 0,
        Phase::RolledBack => 2,
        _ => 1,
    });
}

async fn apply(journal: &mut Journal) -> Result<()> {
    if let Err(error) = journal.plan.validate() {
        return journal.end(Phase::Failed, error.code, error.detail);
    }
    journal.advance(Phase::Stopping)?;
    if let Err(error) = crate::stop_cli::stop(&journal.plan.data_dir, STOP_WAIT).await {
        let ended = match error.downcast_ref::<crate::stop_cli::NotEnded>() {
            Some(_) => end_by_force(journal).await,
            None => Err(format!("{error:#}")),
        };
        if let Err(detail) = ended {
            return journal.end(Phase::Failed, "update.service_did_not_stop", detail);
        }
    }
    let fit = steps::verify_artifact(&journal.plan)
        .and_then(|()| install::preflight(&journal.plan.install_dir, journal.plan.size));
    if let Err(error) = fit {
        return restart_old(journal, error.code, &error.detail).await;
    }
    match journal.plan.kind {
        InstallKind::Msi => {
            if let Err((code, detail)) = install_msi(journal) {
                return restart_old(journal, code, &detail).await;
            }
        }
        _ => {
            if let Err(error) = portable::stage(journal) {
                let _ = tokio::fs::remove_dir_all(journal.staged_dir()).await;
                let code = error
                    .downcast_ref::<InstallError>()
                    .map_or("update.unpack_failed", |error| error.code);
                return restart_old(journal, code, &format!("{error:#}")).await;
            }
            if let Err(error) = portable::switch(journal) {
                return take_back(journal, None, "update.switch_failed", format!("{error:#}"))
                    .await;
            }
        }
    }
    let mut child = match process::start_service(journal, &journal.executable()) {
        Ok(child) => child,
        Err(error) => {
            return take_back(journal, None, "update.start_failed", format!("{error:#}")).await;
        }
    };
    journal.new_started = true;
    journal.write()?;
    let target = journal.plan.target_version.clone();
    match await_health(journal, &mut child, &target, true).await {
        Ok(()) => {
            journal.advance(Phase::Verified)?;
            if let Err(error) = steps::keep_installer(journal) {
                tracing::warn!(%error, "the installer was not kept; the next update cannot reinstall it");
            }
            tracing::info!(version = %target, "the update is installed and answers");
            Ok(())
        }
        Err((code, detail)) => take_back(journal, Some(child), code, detail).await,
    }
}

/// Ends the service that accepted the stop and did not end within [`STOP_WAIT`], and waits until
/// it is gone; why not, when it could not.
///
/// Safe at this point: the backup before the update is written and checked, the service is no
/// longer listening, and a forced end leaves what a crash leaves, which every persistence path
/// survives (`crates/rd-core/recovery-matrix.md`). A version from before 1.8.0-beta.3 never ended
/// with an event stream open (live test 2026-10-01); this one ends by itself within a minute.
async fn end_by_force(journal: &Journal) -> Result<(), String> {
    let (pid, image) = (journal.plan.service_pid, journal.plan.executable.as_str());
    tracing::warn!(
        pid,
        "rDownloader accepted the stop and did not end; it is ended by force"
    );
    process::end_by_force(pid, image).map_err(|error| {
        format!("process {pid} accepted the stop, did not end and could not be ended: {error:#}")
    })?;
    for _ in 0..50 {
        if !process::runs_as(pid, image).unwrap_or(true) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "process {pid} accepted the stop, did not end and was still running after it was ended"
    ))
}

/// `msiexec /i` over the installed version. A failed run is undone by Windows Installer itself.
/// The package is held open as it was checked while `msiexec` reads it.
fn install_msi(journal: &mut Journal) -> Result<(), Failure> {
    let failed = |error: anyhow::Error| ("update.installer_failed", format!("{error:#}"));
    let _checked =
        steps::open_artifact(&journal.plan).map_err(|error| (error.code, error.detail))?;
    journal.advance(Phase::Switching).map_err(failed)?;
    let args = process::msiexec_arguments(
        "/i",
        &journal.plan.artifact,
        &process::msiexec_log(journal, "install"),
        &[],
    );
    match process::run_msiexec(&args).map_err(failed)? {
        code if process::MSI_SUCCESS.contains(&code) => {
            journal.advance(Phase::Switched).map_err(failed)
        }
        code => Err((
            "update.installer_failed",
            format!("msiexec ended with {code}"),
        )),
    }
}

/// Nothing was switched: records why and starts the installed version again.
async fn restart_old(journal: &mut Journal, code: &'static str, detail: &str) -> Result<()> {
    tracing::error!(
        code,
        detail,
        "the update did not go ahead; the installed version starts again"
    );
    journal.end(Phase::Failed, code, detail)?;
    start_and_await(journal, &journal.plan.from_version.clone()).await;
    Ok(())
}

/// The new version failed: stops it, takes the switch back, and starts the old version.
async fn take_back(
    journal: &mut Journal,
    new: Option<Child>,
    code: &'static str,
    detail: String,
) -> Result<()> {
    tracing::error!(code, %detail, "the new version is taken back");
    journal.reason = Some(code.to_owned());
    journal.detail = Some(detail.clone());
    journal.advance(Phase::RollingBack)?;
    if let Some(mut child) = new {
        stop_new(journal, &mut child).await;
    }
    let taken_back = match journal.plan.kind {
        InstallKind::Msi => {
            // Only the package that was kept, as it was kept (security review 2026-09-30,
            // finding 6), held open while Windows Installer reads it.
            let previous = match steps::previous_installer(&journal.plan) {
                Ok(Some(previous)) => Ok(previous),
                Ok(None) => Err("no installer of the previous version was kept".to_owned()),
                Err(error) => Err(format!("the kept installer is refused: {}", error.detail)),
            };
            let (previous, _checked) = match previous {
                Ok(previous) => previous,
                Err(why) => {
                    // No way back through Windows Installer; the new version is started again
                    // so the interface can say so.
                    journal.end(
                        Phase::Failed,
                        "update.msi_rollback_unavailable",
                        format!("{detail}; {why}"),
                    )?;
                    start_and_await(journal, &journal.plan.target_version.clone()).await;
                    return Ok(());
                }
            };
            reinstall(journal, &previous)
        }
        _ => portable::roll_back(journal),
    };
    let taken_back = taken_back.and_then(|()| {
        if journal.new_started {
            steps::restore_database(&journal.plan)?;
        }
        Ok(())
    });
    if let Err(error) = taken_back {
        tracing::error!(%error, "the previous version could not be put back; see the manual recovery in docs/development.md");
        return journal.end(
            Phase::Failed,
            "update.rollback_failed",
            format!("{detail}; taking it back failed: {error:#}"),
        );
    }
    if start_and_await(journal, &journal.plan.from_version.clone()).await {
        journal.end(Phase::RolledBack, code, detail)
    } else {
        journal.end(
            Phase::Failed,
            "update.rollback_failed",
            format!("{detail}; the previous version is back but did not answer"),
        )
    }
}

/// Removes the new package and installs the kept previous one; the `Run` entries the removal
/// takes away are registered again.
fn reinstall(journal: &Journal, previous: &Path) -> Result<()> {
    #[cfg(windows)]
    let registered = [
        rd_autostart::is_registered(rd_autostart::Target::Server),
        rd_autostart::is_registered(rd_autostart::Target::Capture),
    ];
    // The package whose product is removed is the one that was checked and installed.
    let _checked = steps::open_artifact(&journal.plan)?;
    for (step, action, package) in [
        ("remove", "/x", journal.plan.artifact.as_path()),
        ("reinstall", "/i", previous),
    ] {
        let args =
            process::msiexec_arguments(action, package, &process::msiexec_log(journal, step), &[]);
        let code = process::run_msiexec(&args)?;
        anyhow::ensure!(
            process::MSI_SUCCESS.contains(&code),
            "msiexec {action} ended with {code}"
        );
    }
    #[cfg(windows)]
    for (was, program) in registered
        .into_iter()
        .zip(["rdownloader.exe", "rdownloader-capture.exe"])
    {
        if was {
            let status = std::process::Command::new(journal.plan.install_dir.join(program))
                .no_console_window()
                .args(["autostart", "install"])
                .status();
            if !status.is_ok_and(|status| status.success()) {
                tracing::warn!(program, "the login entry could not be registered again");
            }
        }
    }
    Ok(())
}

/// Asks the new version to stop and makes sure its process is gone before its files move.
async fn stop_new(journal: &Journal, child: &mut Child) {
    if let Err(error) = crate::stop_cli::stop(&journal.plan.data_dir, Duration::from_secs(60)).await
    {
        tracing::warn!(%error, "the new version did not stop when asked");
    }
    for _ in 0..50 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Starts the program in the folder and waits for it to answer as `version`; whether it did.
async fn start_and_await(journal: &Journal, version: &str) -> bool {
    match process::start_service(journal, &journal.executable()) {
        Ok(mut child) => match await_health(journal, &mut child, version, false).await {
            Ok(()) => true,
            Err((code, detail)) => {
                tracing::error!(code, %detail, version, "rDownloader did not answer after the update");
                false
            }
        },
        Err(error) => {
            tracing::error!(%error, "rDownloader could not be started after the update");
            false
        }
    }
}

/// Waits until the started process answers its health route with `version`: the address is the
/// one its own local control file names, once that file carries its process id.
async fn await_health(
    journal: &Journal,
    child: &mut Child,
    version: &str,
    is_new: bool,
) -> Result<(), Failure> {
    let limit = Duration::from_secs(journal.plan.health_timeout_secs).max(HEALTH_TIMEOUT);
    let deadline = tokio::time::Instant::now() + limit;
    let client = crate::remote::local_http(Duration::from_secs(5))
        .map_err(|error| ("update.health_timeout", format!("{error:#}")))?;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err((
                "update.new_version_exited",
                format!("{version} ended right after its start ({status})"),
            ));
        }
        if let Ok(Some(control)) = rd_api::local_control::read(&journal.plan.data_dir)
            && control.pid == child.id()
            && answers_as(&client, &control.address, version).await
        {
            if is_new && cfg!(debug_assertions) && std::env::var_os(TEST_FAIL_HEALTH).is_some() {
                return Err((
                    "update.health_failed_test",
                    format!("{TEST_FAIL_HEALTH} declares the new version unhealthy"),
                ));
            }
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err((
                "update.health_timeout",
                format!(
                    "{version} did not answer within {} seconds",
                    limit.as_secs()
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn answers_as(client: &reqwest::Client, address: &str, version: &str) -> bool {
    let Ok(answer) = client
        .get(format!("http://{address}/api/v1/health"))
        .send()
        .await
    else {
        return false;
    };
    answer
        .json::<serde_json::Value>()
        .await
        .is_ok_and(|body| body["version"] == version)
}

/// What `serve` runs before it opens the database: ends or continues the update the journal
/// records (`rd_update::install::recover`). Returns `true` when this process started the restored
/// previous program in its place and must end.
///
/// # Errors
///
/// When the journal is unreadable or a roll-back fails: the program folder may then hold two
/// versions, and nothing may start on it.
pub(crate) fn recover(database: &Path) -> Result<bool> {
    let data = crate::auth_cli::data_directory_of(database);
    let executable = std::env::current_exe().context("locate rDownloader executable")?;
    match recover_at_start(&data, &executable, env!("CARGO_PKG_VERSION"))? {
        Recovery::Continue => Ok(false),
        Recovery::Restart(program) => {
            tracing::warn!(program = %program.display(), "the previous version is back; it starts in this one's place");
            std::process::Command::new(&program)
                .no_console_window()
                .args(std::env::args_os().skip(1))
                .spawn()
                .with_context(|| format!("start {}", program.display()))?;
            Ok(true)
        }
    }
}

/// Once the service answers: an update whose updater is gone is proven by that
/// (`rd_update::install::recover::confirm_started`). Polls only when one waits for its proof.
pub(crate) fn confirm_when_answering(database: &Path, listen: std::net::SocketAddr) {
    let data = crate::auth_cli::data_directory_of(database);
    let waiting = Journal::read(&data)
        .ok()
        .flatten()
        .is_some_and(|journal| journal.phase == Phase::Switched);
    if !waiting {
        return;
    }
    let address = rd_api::local_control::reachable(listen);
    tokio::spawn(async move {
        let Ok(client) = crate::remote::local_http(Duration::from_secs(5)) else {
            return;
        };
        for _ in 0..120 {
            let answered = client
                .get(format!("http://{address}/api/v1/health"))
                .send()
                .await
                .is_ok_and(|answer| answer.status().is_success());
            if answered {
                if let Err(error) = confirm_started(&data, env!("CARGO_PKG_VERSION")) {
                    tracing::warn!(%error, "the update could not be recorded as proven");
                }
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
