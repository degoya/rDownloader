//! Installing an update from the interface (RD-180-02): what
//! `POST /api/v1/system/update/install` starts.
//!
//! Only a portable archive and the Windows installer install themselves (owner, 2026-09-30); every
//! other kind is refused with `update.install_unsupported` and keeps showing its command. The
//! request is checked and answered at once; the steps run behind it and the status follows them:
//!
//! 1. **Download** the offered artifact into `<data>/update/download/`, held to the signed
//!    manifest's size and SHA-256 (`UpdateService::fetch_artifact`) — or take the one a
//!    background download ([`start_download`], `POST /api/v1/system/update/download`) left
//!    there, read and hashed again.
//! 2. **Backup** before the update (RD-180-03, `pre_update_service::prepare`): the checked
//!    database copy always, the encrypted full backup when a passphrase is set up — required
//!    when the manifest's `schema_change` says the release changes the schema, or does not say.
//!    A failure refuses the update.
//! 3. **Hand over**: the journal in phase `handed` with the plan (the artifact, the program
//!    folder, the database and its copy, this process and how it was started), and the updater
//!    started from a copy of this executable. The updater stops this service over the local
//!    control token; from here on the journal speaks for the update.
//!
//! Running downloads refuse the request (`update.transfers_active`) unless the caller agrees:
//! the stop saves the queue, and they continue after the restart.

use std::path::PathBuf;
use std::sync::LazyLock;

use rd_core::AuditAction;
use rd_update::install::{self, Journal, Plan};
use rd_update::{Artifact, InstallKind};

use crate::audit::{AuditContext, AuditEvent};
use crate::dto::{UpdateDownloadStatus, UpdateInstallRequest, UpdateInstallStatus};
use crate::pre_update_service::PreUpdateRequest;
use crate::{ApiError, AppState};

/// Held from the first check of a request to its recorded start, so two requests at once cannot
/// both find nothing running.
static STARTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Checks the request, starts the steps and answers with the status they begin in.
///
/// # Errors
///
/// `409` with `update.install_unsupported`, `update.install_running`, `update.none_available`,
/// `update.no_artifact`, `update.transfers_active`, `update.install_dir_not_writable` or
/// `update.not_enough_space`.
pub async fn start(
    state: &AppState,
    request: UpdateInstallRequest,
    audit: &AuditContext,
) -> Result<UpdateInstallStatus, ApiError> {
    begin(state, request, audit, false).await
}

/// [`start`], recording whether a person asked or the automatic install did (RD-1240-27).
pub(crate) async fn begin(
    state: &AppState,
    request: UpdateInstallRequest,
    audit: &AuditContext,
    automatic: bool,
) -> Result<UpdateInstallStatus, ApiError> {
    let _starting = STARTING.lock().await;
    let updates = &state.updates;
    let kind = updates.install_kind();
    let install_dir = installing_dir(state)?;
    if updates.install_busy() {
        return Err(ApiError::conflict(
            "update.install_running",
            "An update is already being installed",
        ));
    }
    let Offered {
        version: target,
        artifact,
        schema_change,
    } = offered(state).await?;
    let active = state.scheduler.transfer_rates().len();
    if active > 0 && !request.allow_active {
        return Err(ApiError::conflict(
            "update.transfers_active",
            format!(
                "{active} downloads are running; they are saved by the stop and continue after \
                 the restart. Send allow_active to install anyway"
            ),
        )
        .with_param("count", active));
    }
    install::preflight(&install_dir, artifact.size)
        .map_err(|error| ApiError::conflict(error.code, error.detail))?;

    updates.set_progress("downloading", &target, None);
    crate::audit::record(
        state,
        AuditEvent::success(AuditAction::UpdateInstallStarted)
            .by(audit)
            .target("update", &target)
            .detail("from_version", updates.current_version())
            .detail("install_kind", kind.as_str())
            .detail("schema_change", schema_change)
            .detail("active_downloads", active)
            .detail("automatic", automatic),
    )
    .await;
    tracing::info!(target = %target, kind = kind.as_str(), "installing an update");
    let task_state = state.clone();
    let task_audit = audit.clone();
    tokio::spawn(async move {
        let updates = &task_state.updates;
        let release = Release {
            version: &target,
            artifact: &artifact,
            schema_change,
        };
        match run(&task_state, &release, install_dir, &task_audit).await {
            Ok(()) => updates.set_progress("handed", &target, None),
            Err(error) => {
                tracing::error!(code = error.code(), error = %error.message(), "the update was not installed; the installed version keeps running");
                updates.set_progress("failed", &target, Some(error.code().to_owned()));
            }
        }
    });
    state
        .updates
        .install_status()
        .ok_or_else(|| ApiError::from(anyhow::anyhow!("the update's progress was not recorded")))
}

/// Downloads the offered update in the background, for an install that then uses the file
/// (owner, 2026-10-01). Answers at once with where the download stands; a download of the same
/// version that already runs is joined, and a file that is already there and verified is ready
/// in a moment.
///
/// # Errors
///
/// `409` with `update.install_unsupported`, `update.none_available` or `update.no_artifact`: an
/// installation that does not install itself shows its command or the browser's download.
pub async fn start_download(state: &AppState) -> Result<UpdateDownloadStatus, ApiError> {
    installing_dir(state)?;
    let Offered {
        version, artifact, ..
    } = offered(state).await?;
    tracing::info!(target = %version, "downloading an update in the background");
    Ok(state.updates.start_download(&version, &artifact))
}

/// The program folder an update replaces; refused for every kind that does not install itself.
fn installing_dir(state: &AppState) -> Result<PathBuf, ApiError> {
    let kind = state.updates.install_kind();
    state
        .updates
        .install_dir()
        .filter(|_| kind.installs_itself())
        .ok_or_else(|| {
            ApiError::conflict(
                "update.install_unsupported",
                format!(
                    "A {} installation does not install updates itself",
                    kind.as_str()
                ),
            )
            .with_param("kind", kind.as_str())
        })
}

/// The offered version as the stored, verified manifest describes it.
struct Offered {
    version: String,
    artifact: Artifact,
    schema_change: bool,
}

async fn offered(state: &AppState) -> Result<Offered, ApiError> {
    let updates = &state.updates;
    let Some(offered) = updates.status().await.available else {
        return Err(ApiError::conflict(
            "update.none_available",
            "No newer version is available to install",
        ));
    };
    let stored = updates
        .stored()
        .await
        .offer
        .filter(|offer| offer.version == offered.version);
    let schema_change = stored.as_ref().is_none_or(|offer| offer.schema_change);
    let Some(artifact) = stored.and_then(|offer| offer.artifact) else {
        return Err(ApiError::conflict(
            "update.no_artifact",
            "The release has no file for this platform and installation",
        ));
    };
    Ok(Offered {
        version: offered.version,
        artifact,
        schema_change,
    })
}

/// What is installed: the offered version, its artifact, and whether it changes the schema.
struct Release<'a> {
    version: &'a str,
    artifact: &'a Artifact,
    schema_change: bool,
}

/// The steps up to the hand-over; see the module documentation.
async fn run(
    state: &AppState,
    release: &Release<'_>,
    install_dir: PathBuf,
    audit: &AuditContext,
) -> Result<(), ApiError> {
    let (target, artifact) = (release.version, release.artifact);
    let updates = &state.updates;
    let file = updates
        .fetch_artifact(target, artifact)
        .await
        .map_err(|error| ApiError::conflict(error.code(), error.to_string()))?;

    updates.set_progress("preparing", target, None);
    let prepared = crate::pre_update_service::prepare(
        state,
        PreUpdateRequest {
            target_version: target.to_owned(),
            schema_change: release.schema_change,
        },
        audit,
    )
    .await?;

    let plan = plan(
        state,
        target,
        artifact,
        file,
        install_dir,
        &prepared.database_copy.path,
    )?;
    plan.validate()
        .map_err(|error| ApiError::conflict(error.code, error.detail))?;
    let mut journal = Journal::begin(plan);
    journal
        .write()
        .map_err(|error| failed("update.plan_invalid", &error))?;
    updates.set_progress("handed", target, None);
    if let Err(error) = updates.launch(&journal) {
        if let Err(journal_error) = journal.end(
            install::Phase::Failed,
            "update.updater_failed",
            format!("{error:#}"),
        ) {
            tracing::warn!(
                error = %format!("{journal_error:#}"),
                "the failed update could not be recorded in its journal"
            );
        }
        return Err(failed("update.updater_failed", &error));
    }
    tracing::info!(target = %target, "the updater was started and stops this service next");
    Ok(())
}

/// What the updater needs, every path absolute.
fn plan(
    state: &AppState,
    target: &str,
    artifact: &Artifact,
    file: PathBuf,
    install_dir: PathBuf,
    database_copy: &str,
) -> Result<Plan, ApiError> {
    let updates = &state.updates;
    let kind = updates.install_kind();
    let absolute =
        |path: &std::path::Path| std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "rdownloader.exe".to_owned()
            } else {
                "rdownloader".to_owned()
            }
        });
    // The service's own arguments, so the new version starts as this one did; one that is no
    // text cannot be written into the journal, and guessing it could start another database.
    let mut service_args = Vec::new();
    for argument in std::env::args_os().skip(1) {
        service_args.push(argument.into_string().map_err(|argument| {
            ApiError::conflict(
                "update.install_unsupported",
                format!("the service argument {argument:?} is not text"),
            )
        })?);
    }
    let service_cwd = std::env::current_dir()
        .map_err(|error| failed("update.plan_invalid", &anyhow::Error::new(error)))?;
    let data = updates.data_dir();
    let previous_installer = (kind == InstallKind::Msi)
        .then(|| install::steps::kept_installer(&data, updates.current_version()))
        .flatten();
    Ok(Plan {
        kind,
        from_version: updates.current_version().to_owned(),
        target_version: target.to_owned(),
        artifact: absolute(&file),
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        install_dir: absolute(&install_dir),
        executable,
        database: absolute(state.database.path()),
        database_copy: Some(absolute(std::path::Path::new(database_copy))),
        service_pid: std::process::id(),
        service_args,
        service_cwd,
        health_timeout_secs: install::DEFAULT_HEALTH_TIMEOUT_SECS,
        previous_installer_sha256: previous_installer
            .as_deref()
            .and_then(install::steps::kept_installer_sha256),
        previous_installer,
        data_dir: data,
    })
}

fn failed(code: &'static str, error: &anyhow::Error) -> ApiError {
    ApiError::conflict(code, format!("{error:#}"))
}
