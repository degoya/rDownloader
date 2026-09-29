//! A sealed archive on its way to every destination, and retention behind it (RD-160-02).
//!
//! The service's side: the context a destination opens with, from the live settings, and the
//! destination a stored row names. The delivery itself, the ledger and retention are
//! `rd_backup::ledger`, where their crash points are (RD-170-07).

use chrono::Utc;
pub use rd_backup::ledger::{Delivered, Target, label};
use rd_backup::{
    BackupDestination, DestinationConfig, DestinationContext, DestinationError, RetryPolicy,
    SealedBackup,
};
use rd_db::BackupDestinationRecord;

use crate::AppState;

/// What opening a destination needs, read from the live settings.
pub async fn destination_context(state: &AppState) -> DestinationContext {
    let postprocess = match state
        .database
        .service_settings::<rd_core::PostprocessSettings>()
        .await
    {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "the post-processing settings could not be read; rclone is looked up on PATH");
            rd_core::PostprocessSettings::default()
        }
    };
    DestinationContext {
        object_storage: state.object_storage.clone(),
        rclone_executable: postprocess.rclone_executable,
        vendor_directory: postprocess.vendor_directory,
        bandwidth: state.scheduler.bandwidth().upload_limiter(),
    }
}

/// Opens a stored destination for a run or a verification.
///
/// # Errors
///
/// When the row names no destination this build knows, or it cannot serve.
pub async fn open(
    context: &DestinationContext,
    record: &BackupDestinationRecord,
) -> Result<Box<dyn BackupDestination>, DestinationError> {
    let Some(config) = DestinationConfig::parse(&record.kind, &record.config) else {
        return Err(DestinationError::Misconfigured {
            code: rd_backup::remote::DESTINATION_KIND_UNKNOWN,
            detail: format!("destination kind {} is not supported", record.kind),
        });
    };
    config.open(context).await
}

/// Delivers `sealed` to every opened target at once, records each archive in the ledger,
/// prunes behind it and finishes each destination's row (`rd_backup::ledger::deliver`).
pub async fn deliver(
    state: &AppState,
    run_id: &str,
    instance_id: &str,
    sealed: &SealedBackup,
    created_at: chrono::DateTime<Utc>,
    targets: Vec<Target>,
    policy: RetryPolicy,
) -> Vec<Delivered> {
    rd_backup::ledger::deliver(
        &state.database,
        rd_backup::ledger::Run {
            id: run_id,
            instance_id,
            sealed,
            created_at,
        },
        targets,
        policy,
    )
    .await
}
