//! The self-update's side of [`UpdateService`] (RD-180-02): where the program lives, how the
//! updater starts, and what the status says while an update runs and after it ended.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use rd_update::{
    Fetcher, InstallKind, UpdateError,
    install::{self, Journal, Phase},
};

use super::{Installation, Launcher, Progress, UpdateService};
use crate::dto::UpdateInstallStatus;

/// How long a handed-over update may wait for its updater to take the lock before the status
/// calls it failed.
const HANDOVER_GRACE: chrono::Duration = chrono::Duration::seconds(60);
/// How long the outcome of a finished update is shown.
const OUTCOME_SHOWN_FOR: chrono::Duration = chrono::Duration::days(7);

impl UpdateService {
    /// Installs as `kind` from `directory` and starts the updater with `launcher`.
    ///
    /// For tests only, which have no installed program and must not start a process; the
    /// service never calls it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_installation(&self, kind: InstallKind, directory: PathBuf, launcher: Launcher) {
        if let Ok(mut installation) = self.0.installation.write() {
            *installation = Installation {
                kind,
                directory: Some(directory),
                launcher,
            };
        }
    }

    /// How this installation was installed.
    #[must_use]
    pub fn install_kind(&self) -> InstallKind {
        self.0
            .installation
            .read()
            .map_or(InstallKind::Unknown, |installation| installation.kind)
    }

    /// The running program's folder, which a portable update replaces.
    #[must_use]
    pub fn install_dir(&self) -> Option<PathBuf> {
        self.0
            .installation
            .read()
            .ok()
            .and_then(|installation| installation.directory.clone())
    }

    /// The running version.
    #[must_use]
    pub fn current_version(&self) -> &str {
        &self.0.current
    }

    /// Where the manifests and the artifacts are fetched.
    ///
    /// # Errors
    ///
    /// When the source lock is poisoned.
    pub fn fetcher(&self) -> Result<Arc<dyn Fetcher>, UpdateError> {
        self.0
            .source
            .read()
            .map(|source| Arc::clone(&source.fetcher))
            .map_err(|_| UpdateError::Other(anyhow::anyhow!("update source lock poisoned")))
    }

    /// The data directory, absolute: the database's folder.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        let folder = self
            .0
            .database
            .path()
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
        std::path::absolute(&folder).unwrap_or(folder)
    }

    /// Starts the updater for `journal`.
    ///
    /// # Errors
    ///
    /// When the copy or the start of the updater fails.
    pub fn launch(&self, journal: &Journal) -> anyhow::Result<()> {
        let launcher = self
            .0
            .installation
            .read()
            .map(|installation| Arc::clone(&installation.launcher))
            .map_err(|_| anyhow::anyhow!("update installation lock poisoned"))?;
        launcher(journal)
    }

    /// Records what the service does for an update of `target` before the updater takes over:
    /// `downloading`, `preparing`, `handed` or `failed` (with `reason`).
    pub fn set_progress(&self, state: &'static str, target: &str, reason: Option<String>) {
        let now = Utc::now();
        if let Ok(mut progress) = self.0.progress.lock() {
            let started_at = progress
                .as_ref()
                .filter(|progress| progress.target_version == target && state != "downloading")
                .map_or(now, |progress| progress.started_at);
            *progress = Some(Progress {
                state,
                target_version: target.to_owned(),
                reason,
                started_at,
                updated_at: now,
            });
        }
    }

    /// Where an update stands, or how the last one ended; see the module documentation.
    #[must_use]
    pub fn install_status(&self) -> Option<UpdateInstallStatus> {
        let progress = self
            .0
            .progress
            .lock()
            .ok()
            .and_then(|progress| progress.clone());
        if let Some(progress) = progress.filter(|progress| progress.state != "handed") {
            return Some(UpdateInstallStatus {
                state: progress.state.to_owned(),
                from_version: self.0.current.clone(),
                target_version: progress.target_version,
                reason: progress.reason,
                started_at: progress.started_at.to_rfc3339(),
                updated_at: progress.updated_at.to_rfc3339(),
            });
        }
        let data = self.data_dir();
        let journal = Journal::read(&data).ok().flatten()?;
        let now = Utc::now();
        if journal.phase.is_terminal() && now - journal.updated_at > OUTCOME_SHOWN_FOR {
            return None;
        }
        let mut reason = journal.reason.clone();
        let state = match journal.phase {
            Phase::Handed | Phase::Stopping
                if !install::updater_running(&data)
                    && now - journal.updated_at > HANDOVER_GRACE =>
            {
                reason = Some("update.updater_did_not_start".to_owned());
                "failed"
            }
            Phase::Handed | Phase::Stopping => "restarting",
            Phase::Staged | Phase::Switching => "installing",
            Phase::Switched => "verifying",
            Phase::RollingBack => "rolling_back",
            Phase::Verified => "done",
            Phase::RolledBack => "rolled_back",
            Phase::Failed => "failed",
        };
        Some(UpdateInstallStatus {
            state: state.to_owned(),
            from_version: journal.plan.from_version,
            target_version: journal.plan.target_version,
            reason,
            started_at: journal.started_at.to_rfc3339(),
            updated_at: journal.updated_at.to_rfc3339(),
        })
    }

    /// Whether an update is under way, from the service's first step to the updater's last.
    #[must_use]
    pub fn install_busy(&self) -> bool {
        self.install_status().is_some_and(|status| {
            !matches!(status.state.as_str(), "done" | "rolled_back" | "failed")
        })
    }
}
