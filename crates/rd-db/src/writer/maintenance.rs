//! Service-wide operations that belong to no single domain: the settings blob, the whole-file
//! restore from `backup_store`, the WAL checkpoint, the rewrite without free pages, the
//! consistent copy a full backup is built from, the interrupted-work sweep and the event
//! retention sweep.

use super::{Writer, purge_old_events, send};
use crate::commands::MaintenanceCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_maintenance(&mut self, command: MaintenanceCommand) {
        match command {
            MaintenanceCommand::ReplaceConfig { replacement, reply } => {
                let result =
                    crate::backup_store::replace_all(&mut self.connection, replacement).await;
                if let Ok(outcome) = &result {
                    for event in &outcome.events {
                        let _ = self.events.send(event.clone());
                    }
                }
                send(reply, result.map(|outcome| outcome.released_secrets));
            }
            MaintenanceCommand::SetSetting { key, value, reply } => {
                send(reply, self.set_setting(&key, &value).await);
            }
            MaintenanceCommand::InsertSettingIfAbsent { key, value, reply } => {
                send(reply, self.insert_setting_if_absent(&key, &value).await);
            }
            MaintenanceCommand::CheckpointWal { reply } => {
                send(reply, self.checkpoint_wal().await);
            }
            MaintenanceCommand::RecoverInterrupted { reply } => {
                send(reply, self.recover_interrupted().await);
            }
            MaintenanceCommand::PurgeOldEvents { reply } => {
                send(reply, purge_old_events(&mut self.connection).await);
            }
            MaintenanceCommand::PruneTransferStats { retention, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::stats_store::prune(&mut self.connection, retention, now).await,
                );
            }
            MaintenanceCommand::ClearTransferStats { reply } => {
                send(reply, crate::stats_store::clear(&mut self.connection).await);
            }
            MaintenanceCommand::Vacuum { reply } => {
                let result = sqlx::query("VACUUM")
                    .execute(&mut self.connection)
                    .await
                    .map(|_| ())
                    .map_err(anyhow::Error::from);
                send(reply, result);
            }
            MaintenanceCommand::ReclaimFreePages { reply } => {
                send(reply, self.reclaim_free_pages().await);
            }
            MaintenanceCommand::VacuumInto { path, reply } => {
                send(reply, self.vacuum_into(&path).await);
            }
        }
    }
}
