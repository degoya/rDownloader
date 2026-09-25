//! Thins the persistent transfer statistics on a schedule (RD-110-01).
//!
//! Hourly buckets older than the hourly window are folded into their day, and buckets older
//! than the retention are deleted — in batches of `rd_db::PRUNE_BATCH` rows through the
//! serialized writer, with a pause between two batches, so the queue's own writes are never
//! held for longer than one batch takes. The settings are re-read every pass, like the other
//! supervisors, so a change applies without a restart.

use std::time::Duration;

use crate::AppState;

/// Hourly: the windows are measured in days, and a pass that finds nothing costs one query.
const TICK: Duration = Duration::from_secs(3_600);
/// The first pass waits a minute so start-up is not competing with the sweep.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(60);
/// Breather between two batches, so a burst of queue writes gets the writer in between.
const BETWEEN_BATCHES: Duration = Duration::from_millis(50);

/// Starts the sweep loop. Ends with the application state, like the other supervisors.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            match run_once(&state).await {
                Ok((downsampled, deleted)) if downsampled + deleted > 0 => {
                    tracing::info!(downsampled, deleted, "transfer statistics thinned");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "transfer statistics could not be thinned");
                }
            }
        }
    });
}

/// One full sweep, batch by batch; returns what it folded and what it deleted.
pub(crate) async fn run_once(state: &AppState) -> anyhow::Result<(u64, u64)> {
    let settings = match crate::handlers::stored_settings(&state.database).await {
        Ok(settings) => settings,
        Err(error) => anyhow::bail!("{}", error.message()),
    };
    let retention = crate::stats_handlers::retention_of(&settings);
    let (mut downsampled, mut deleted) = (0, 0);
    loop {
        let report = state.database.prune_transfer_stats(retention).await?;
        downsampled += report.downsampled;
        deleted += report.deleted;
        if !report.more {
            return Ok((downsampled, deleted));
        }
        tokio::time::sleep(BETWEEN_BATCHES).await;
    }
}
