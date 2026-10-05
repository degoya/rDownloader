//! Settings that change while the service runs, and the transfer kinds switched off.

use std::sync::atomic::Ordering;

use anyhow::{Context, Result};

use crate::{
    BlockReason, MAX_AUTO_RETRY_INTERVAL_HOURS, MAX_AUTO_RETRY_ROUNDS, MAX_CONFIGURABLE_RETRIES,
    MAX_EXTERNAL_PARALLEL_FILES, MIN_AUTO_RETRY_INTERVAL_HOURS, RuntimeSettings, SchedulerHandle,
};

impl SchedulerHandle {
    /// Applies queue, chunk and bandwidth settings to the running service.
    pub fn validate_runtime_settings(settings: &RuntimeSettings) -> Result<()> {
        anyhow::ensure!(
            settings.max_active_files > 0,
            "max_active_files must be positive"
        );
        anyhow::ensure!(
            settings.max_chunks_per_file > 0,
            "max_chunks_per_file must be positive"
        );
        anyhow::ensure!(
            settings.external_connections_per_file <= 32,
            "external_connections_per_file must be between 0 and 32"
        );
        anyhow::ensure!(
            settings.external_parallel_files <= MAX_EXTERNAL_PARALLEL_FILES,
            "external_parallel_files must be between 0 and {MAX_EXTERNAL_PARALLEL_FILES}"
        );
        anyhow::ensure!(
            settings.max_connections_per_host <= rd_http::MAX_CONNECTIONS_PER_HOST,
            "max_connections_per_host must not exceed {}",
            rd_http::MAX_CONNECTIONS_PER_HOST
        );
        anyhow::ensure!(
            settings.max_retries <= MAX_CONFIGURABLE_RETRIES,
            "max_retries must not exceed {MAX_CONFIGURABLE_RETRIES}"
        );
        anyhow::ensure!(
            (MIN_AUTO_RETRY_INTERVAL_HOURS..=MAX_AUTO_RETRY_INTERVAL_HOURS)
                .contains(&settings.auto_retry_interval_hours),
            "auto_retry_interval_hours must be between {MIN_AUTO_RETRY_INTERVAL_HOURS} and \
             {MAX_AUTO_RETRY_INTERVAL_HOURS}"
        );
        anyhow::ensure!(
            settings.auto_retry_max_rounds <= MAX_AUTO_RETRY_ROUNDS,
            "auto_retry_max_rounds must not exceed {MAX_AUTO_RETRY_ROUNDS}"
        );
        if let Some(certificate) = settings
            .custom_ca_pem
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            reqwest::Certificate::from_pem(certificate.as_bytes())
                .context("invalid custom CA certificate")?;
        }
        Ok(())
    }

    /// Applies validated queue, chunk and network settings to the running service.
    pub async fn update_runtime_settings(&self, settings: RuntimeSettings) -> Result<()> {
        Self::validate_runtime_settings(&settings)?;
        let custom_ca_pem = settings
            .custom_ca_pem
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.into_bytes())
            .into_iter()
            .collect::<Vec<_>>();
        self.max_active_files
            .store(settings.max_active_files, Ordering::Release);
        self.max_chunks_per_file
            .store(settings.max_chunks_per_file, Ordering::Release);
        self.external_connections_per_file
            .store(settings.external_connections_per_file, Ordering::Release);
        self.external_parallel_files
            .store(settings.external_parallel_files, Ordering::Release);
        self.host_limits
            .set_limit(settings.max_connections_per_host);
        self.max_retries
            .store(settings.max_retries, Ordering::Release);
        self.generate_sha256
            .store(settings.generate_sha256, Ordering::Release);
        self.pause_during_postprocess
            .store(settings.pause_during_postprocess, Ordering::Release);
        self.auto_retry_failed
            .store(settings.auto_retry_failed, Ordering::Release);
        self.auto_retry_interval_hours
            .store(settings.auto_retry_interval_hours, Ordering::Release);
        self.auto_retry_max_rounds
            .store(settings.auto_retry_max_rounds, Ordering::Release);
        // Re-enabling a kind has to release what disabling it blocked; otherwise switching a
        // service back on leaves its jobs sitting in `Blocked` with no way to notice.
        let released: Vec<rd_core::DownloadKind> = {
            let mut disabled = self.disabled_kinds.lock().await;
            let released = disabled
                .iter()
                .copied()
                .filter(|kind| !settings.disabled_kinds.contains(kind))
                .collect();
            *disabled = settings.disabled_kinds.clone();
            released
        };
        if !released.is_empty() {
            self.requeue_blocked_of_kinds(&released).await;
        }
        // The hand-set limits are independent of the schedule: they stay in force through
        // every profile switch, and whichever of the two is stricter wins.
        let limits = self.config.bandwidth.limits();
        limits.set_manual_limit(settings.speed_limit_bytes_per_second);
        limits.set_manual_upload_limit(settings.upload_limit_bytes_per_second);
        {
            let mut defaults = self.network_defaults.write().await;
            defaults.global_proxy_profile_id = settings.global_proxy_profile_id;
            defaults.custom_ca_pem = custom_ca_pem;
            defaults.tls_revision = defaults.tls_revision.wrapping_add(1);
        }
        self.clients.clear().await;
        Ok(())
    }

    /// Requeues what disabling those kinds had blocked — and only that.
    ///
    /// The kind alone is not enough of a filter: a file of a re-enabled kind may also be
    /// blocked because its storage root is full or because its validators changed mid-transfer,
    /// and switching the kind back on is not a verdict on either of those.
    async fn requeue_blocked_of_kinds(&self, kinds: &[rd_core::DownloadKind]) {
        let blocked = match self
            .database
            .downloads_blocked_by(BlockReason::KindDisabled.as_str())
            .await
        {
            Ok(blocked) => blocked,
            Err(error) => {
                tracing::warn!(%error, "jobs blocked by a disabled kind were not read");
                return;
            }
        };
        for id in blocked {
            let file = match self.database.get_download(id).await {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, download = %id, "a blocked job could not be read");
                    continue;
                }
            };
            if !kinds.contains(&file.kind) {
                continue;
            }
            if let Err(error) = self.resume(file.id).await {
                tracing::warn!(%error, download = %file.id, "could not resume a re-enabled kind");
            }
        }
    }
}
