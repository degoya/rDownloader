//! Service-wide operations that belong to no single domain: the settings blob, the whole-file
//! restore from `backup_store`, the WAL checkpoint, the interrupted-work sweep and the event
//! retention sweep.

use super::{Writer, purge_old_events, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_maintenance(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::ReplaceConfig { replacement, reply } => {
                let result =
                    crate::backup_store::replace_all(&mut self.connection, replacement).await;
                if let Ok(events) = &result {
                    for event in events {
                        let _ = self.events.send(event.clone());
                    }
                }
                send(reply, result.map(|_| ()));
            }
            WriterCommand::SetSetting { key, value, reply } => {
                send(reply, self.set_setting(&key, &value).await);
            }
            WriterCommand::CheckpointWal { reply } => {
                send(reply, self.checkpoint_wal().await);
            }
            WriterCommand::RecoverInterrupted { reply } => {
                send(reply, self.recover_interrupted().await);
            }
            WriterCommand::PurgeOldEvents { reply } => {
                send(reply, purge_old_events(&mut self.connection).await);
            }
            WriterCommand::PruneTransferStats { retention, reply } => {
                let now = chrono::Utc::now();
                send(
                    reply,
                    crate::stats_store::prune(&mut self.connection, retention, now).await,
                );
            }
            WriterCommand::ClearTransferStats { reply } => {
                send(reply, crate::stats_store::clear(&mut self.connection).await);
            }
            // `Writer::run` routes every variant to exactly one handler, and its match is
            // exhaustive over `WriterCommand`, so nothing reaches this arm. It drops the
            // command instead of panicking: a mis-routed command must not take down the one
            // task every mutation in the process runs on, and the caller already treats a
            // dropped reply as a failed request.
            _ => {}
        }
    }
}
