//! Writing one archive: the key, the sources and the retried write to the destination.

use super::*;

/// What a run that reached at least one destination wrote.
pub(super) struct Written {
    pub(super) archive_name: String,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
    pub(super) parts: Vec<rd_backup::ManifestPart>,
    /// Destinations that have the archive.
    pub(super) delivered: usize,
    /// `<destination>: <code>` of each destination that does not.
    pub(super) failed: Vec<String>,
}

/// How many times and how patiently a destination is tried within one run.
pub(super) const RETRY: RetryPolicy = RetryPolicy {
    attempts: 3,
    first_delay: Duration::from_secs(30),
};

/// Everything a run needs from the service: the key, the destinations, the settings bundle,
/// then `rd_backup::seal_backup` and the deliveries.
pub(super) async fn produce(
    state: &AppState,
    config: BackupConfig,
    run_id: &str,
    started_at: DateTime<Utc>,
) -> Result<Written, BackupError> {
    // No key, no backup: there is no path that writes one unsealed.
    let record = config
        .key
        .as_ref()
        .ok_or_else(|| failure("backup.key_missing", "no backup passphrase has been set up"))?;
    let key = load_key(state, record).await?;

    let records: Vec<_> = config
        .destinations
        .into_iter()
        .filter(|record| record.enabled)
        .collect();
    if records.is_empty() {
        return Err(failure(
            "backup.destination_missing",
            "no backup destination is set",
        ));
    }
    // Opened before anything is written: when none of them can serve, the run ends with the
    // first one's reason and no archive is sealed for nobody.
    let context = backup_delivery::destination_context(state).await;
    let mut targets = Vec::with_capacity(records.len());
    for record in records {
        let opened = backup_delivery::open(&context, &record).await;
        targets.push(Target { record, opened });
    }
    if targets.iter().all(|target| target.opened.is_err()) {
        let mut first = None;
        for target in &targets {
            if let Err(error) = &target.opened {
                finish_failed(
                    state,
                    run_id,
                    &target.record.id,
                    error.code(),
                    error.to_string(),
                )
                .await;
                if first.is_none() {
                    first = Some(BackupError {
                        code: error.code(),
                        detail: error.to_string(),
                    });
                }
            }
        }
        return Err(first.unwrap_or_else(|| {
            failure("backup.destination_missing", "no backup destination is set")
        }));
    }

    let sources = backup_sources(state, &key, &config.instance_id).await?;
    let sealed = rd_backup::seal_backup(
        &state.database,
        sources,
        &key,
        &rd_backup::staging_root(&data_directory(state)),
        run_id,
        started_at,
    )
    .await?;
    let deliveries = backup_delivery::deliver(
        state,
        run_id,
        &config.instance_id,
        &sealed,
        started_at,
        targets,
        RETRY,
    )
    .await;
    let mut failed = Vec::new();
    let mut first_failure = None;
    for Delivered { label, result } in &deliveries {
        if let Err((code, detail)) = result {
            failed.push(format!("{label}: {code}"));
            if first_failure.is_none() {
                first_failure = Some((*code, detail.clone()));
            }
        }
    }
    let delivered = deliveries.len() - failed.len();
    if delivered == 0 {
        let (code, detail) = first_failure.unwrap_or(("backup.destination_failed", String::new()));
        return Err(BackupError { code, detail });
    }
    Ok(Written {
        archive_name: sealed.archive_name,
        size_bytes: sealed.size_bytes,
        sha256: sealed.sha256,
        parts: sealed.manifest.parts,
        delivered,
        failed,
    })
}

/// The configured key, out of the secret store. Also what the backup before an update seals
/// with (`crate::pre_update_service`, RD-180-03).
pub(crate) async fn load_key(
    state: &AppState,
    record: &rd_db::BackupKeyRecord,
) -> Result<BackupKey, BackupError> {
    let key_bytes = state
        .secrets
        .get_bytes(&record.reference)
        .await
        .map_err(|error| failure("backup.key_unavailable", format!("{error:#}")))?;
    let salt = STANDARD
        .decode(&record.salt)
        .map_err(|error| failure("backup.key_unavailable", error.to_string()))?;
    BackupKey::from_stored(&key_bytes, &salt)
        .map_err(|error| failure("backup.key_unavailable", format!("{error:#}")))
}

/// What an archive holds besides the database: the settings bundle sealed under `key`, the
/// torrent session and the stored `.torrent` files.
pub(crate) async fn backup_sources(
    state: &AppState,
    key: &BackupKey,
    instance_id: &str,
) -> Result<BackupSources, BackupError> {
    let bundle = build_settings_bundle(state, SecretSealing::BackupKey(key))
        .await
        .map_err(|error| failure("backup.settings_failed", error.message()))?;
    let settings_bundle = serde_json::to_vec_pretty(&bundle)
        .map_err(|error| failure("backup.settings_failed", error.to_string()))?;
    Ok(BackupSources {
        settings_bundle,
        torrent_session: Some(state.torrent.session_directory()),
        torrent_files: Some(state.torrent.torrent_file_directory()),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        instance_id: instance_id.to_owned(),
    })
}
