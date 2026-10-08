//! Starting the scheduler, with the recovery of what the previous run left, and stopping it.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicUsize},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use rd_db::Database;
use rd_http::{ClientPool, HostLimits};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    DEFAULT_AUTO_RETRY_INTERVAL_HOURS, DEFAULT_AUTO_RETRY_MAX_ROUNDS, DEFAULT_MAX_RETRIES,
    ExternalRunner, SchedulerConfig, SchedulerHandle, account_traffic, active::ActiveState, holds,
    hostblock, rates, runner,
};

/// Marks the start's storage recovery as run when it is dropped, panic or not.
struct RecoveryFinished(Arc<tokio::sync::watch::Sender<bool>>);

impl Drop for RecoveryFinished {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

impl SchedulerHandle {
    /// Waits until the start's background storage work has run: the moves the previous run
    /// left are carried on and the content index is checked. A move set up before that may be
    /// finished by it, which a test that interrupts a move of its own has to rule out.
    pub async fn storage_recovery_finished(&self) {
        let mut done = self.storage_recovered.subscribe();
        // A sender that is gone has nothing left to wait for.
        let _ = done.wait_for(|finished| *finished).await;
    }

    /// Recovers interrupted jobs and starts queue supervision.
    pub async fn start(
        database: Database,
        config: SchedulerConfig,
        secrets: rd_secrets::SecretStore,
        plugins: Option<&rd_plugin_host::PluginTypeRegistry>,
        runners: Vec<Arc<dyn ExternalRunner>>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(&config.downloads_directory)
            .await
            .context("create downloads directory")?;
        database.recover_interrupted().await?;
        let interrupted = database.interrupt_storage_operations().await?;
        if interrupted > 0 {
            tracing::info!(
                interrupted,
                "storage operations of the previous run were interrupted"
            );
        }
        // `session_store::purge_expired` implements a 30-day grace period that had no caller
        // outside its own test, so the `sessions` table grew for the life of the install —
        // silently, because `list_sessions` filters expired rows out anyway. rd-db owns no
        // periodic task of its own, so the sweep runs here, beside the other thing that has to
        // happen once before any work is dispatched.
        let purged = database
            .purge_expired_sessions(database.session_limits().await?)
            .await?;
        if purged > 0 {
            tracing::info!(purged, "removed sessions past their grace period");
        }
        // The same reasoning for the event log: append-only, read by nothing, and without a
        // sweep the largest table in the file on any install that has been running a while.
        let purged_events = database.purge_old_events().await?;
        if purged_events > 0 {
            tracing::info!(purged_events, "removed events past their retention");
        }
        let clients = ClientPool::default();
        let network_defaults = config.network_defaults.clone();
        let captcha = rd_captcha::CaptchaBroker::new(database.clone(), secrets.clone());
        let mut resolvers = rd_plugin_host::ResolverService::new(
            database.clone(),
            clients.clone(),
            secrets.clone(),
            network_defaults.clone(),
            Some(Arc::new(captcha.clone())),
            // The service's own listeners are no plugin's to reach (RA-HOST-01).
            rd_plugin_host::OwnEndpoints::new(config.own_address),
        );
        if let Some(plugins) = plugins {
            let loaded = resolvers.load_components_from_registry(plugins).await?;
            tracing::info!(loaded, "loaded installed resolver components");
        }
        // The twelfth world runs on the same narrowed host a resolver does, so a
        // stream-transform plugin reaches its own manifest's domains and nothing else.
        let transforms = plugins.map_or_else(
            rd_plugin_host::extension::StreamTransformProviders::none,
            |plugins| {
                rd_plugin_host::extension::StreamTransformProviders::from_registry(
                    plugins,
                    Some(resolvers.host()),
                )
            },
        );
        if !transforms.is_empty() {
            tracing::info!(
                loaded = transforms.list().len(),
                "loaded installed stream-transform components"
            );
        }
        // Read before `config` is moved into the handle, and shared by every transfer so
        // the budget belongs to the host rather than to one download.
        let host_limits = HostLimits::new(config.max_connections_per_host);
        let handle = Self {
            database,
            max_active_files: Arc::new(AtomicUsize::new(config.max_active_files)),
            max_retries: Arc::new(AtomicU32::new(DEFAULT_MAX_RETRIES)),
            max_chunks_per_file: Arc::new(AtomicUsize::new(config.max_chunks_per_file)),
            external_connections_per_file: Arc::new(AtomicUsize::new(0)),
            external_parallel_files: Arc::new(AtomicUsize::new(config.external_parallel_files)),
            generate_sha256: Arc::new(AtomicBool::new(true)),
            pause_during_postprocess: Arc::new(AtomicBool::new(true)),
            auto_retry_failed: Arc::new(AtomicBool::new(false)),
            auto_retry_interval_hours: Arc::new(AtomicU32::new(DEFAULT_AUTO_RETRY_INTERVAL_HOURS)),
            auto_retry_max_rounds: Arc::new(AtomicU32::new(DEFAULT_AUTO_RETRY_MAX_ROUNDS)),
            auto_retry_was_enabled: Arc::new(AtomicBool::new(true)),
            disabled_kinds: Arc::new(Mutex::new(Vec::new())),
            network_defaults,
            captcha,
            config: Arc::new(config),
            clients,
            resolvers,
            transforms: Arc::new(transforms),
            secrets,
            active: Arc::new(Mutex::new(ActiveState::default())),
            network_hold: Arc::new(holds::Holds::default()),
            queue_pause: Arc::new(Mutex::new(None)),
            host_blocks: hostblock::HostBlocks::default(),
            traffic_holds: account_traffic::TrafficHolds::default(),
            host_limits,
            provider_slots: Arc::new(Mutex::new(HashMap::new())),
            free_slots: Arc::new(Mutex::new(HashMap::new())),
            runners: Arc::new(runner::RunnerRegistry::new(runners)),
            rates: Arc::new(rates::RateSampler::default()),
            relocations: Arc::new(tokio::sync::Mutex::new(())),
            storage_recovered: Arc::new(tokio::sync::watch::Sender::new(false)),
            shutdown: CancellationToken::new(),
            #[cfg(test)]
            queue_reads: Arc::new(AtomicUsize::new(0)),
        };
        // Closes `scheduler.before_mirror_promoted`: a group whose active member failed while
        // its successor had not been promoted yet holds nothing and is dispatched by nobody,
        // because the dispatcher only ever looks at rows that are already queued.
        if let Err(error) = handle.recover_stalled_mirror_groups().await {
            tracing::warn!(%error, "stalled mirror groups could not be restarted");
        }
        if let Err(error) = handle.restore_capacity().await {
            tracing::warn!(%error, "storage capacity state could not be restored");
        }
        if let Err(error) = handle.reload_bandwidth().await {
            tracing::warn!(%error, "bandwidth profiles could not be loaded");
        }
        // Before the first dispatch, so a file the pause holds does not start in the gap.
        if let Err(error) = handle.restore_queue_pause().await {
            tracing::warn!(%error, "the timed queue pause could not be restored");
        }
        // The same for an account whose traffic is used up (RD-1190-14).
        if let Err(error) = handle.restore_account_traffic().await {
            tracing::warn!(%error, "the accounts waiting for traffic could not be restored");
        }
        // Storage work the previous run left: category moves to finish, the history to
        // settle, the content index to check against the disk (RD-150-02). In the background,
        // because a cross-device move is a copy and the queue must not wait for it.
        {
            let recovering = handle.clone();
            tokio::spawn(async move {
                // Sent on the way out however the work ends: a panic in it left every
                // `storage_recovery_finished` waiting for good (audit 1.9.1, T13).
                let _finished = RecoveryFinished(Arc::clone(&recovering.storage_recovered));
                recovering.recover_storage_work().await;
            });
        }
        tokio::spawn(handle.clone().supervise());
        Ok(handle)
    }

    /// Restarts mirror groups that were left with nobody running; see
    /// [`crate::failures::recover_stalled_mirror_groups`].
    async fn recover_stalled_mirror_groups(&self) -> Result<()> {
        crate::failures::recover_stalled_mirror_groups(self).await
    }

    /// Stops new work, checkpoints active jobs and truncates the WAL.
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.cancel();
        let tokens = {
            let active = self.active.lock().await;
            active.tokens.values().cloned().collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if self.active.lock().await.tokens.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.database.checkpoint_wal().await
    }
}
