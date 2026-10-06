//! The writer half of `log_store`: the batched append and the bounded prune.

use super::{Writer, send};
use crate::commands::LogsCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_logs(&mut self, command: LogsCommand) {
        match command {
            LogsCommand::AppendLogRecords { records, reply } => {
                let result =
                    crate::log_store::append_log_records(&mut self.connection, &records).await;
                send(reply, result);
            }
            LogsCommand::PruneLogRecords {
                max_records,
                older_than,
                batch,
                reply,
            } => {
                let result = crate::retention::prune_records(
                    &mut self.connection,
                    crate::retention::RecordTable::Log,
                    max_records,
                    older_than,
                    batch,
                )
                .await;
                send(reply, result);
            }
            LogsCommand::ClearLogRecords { reply } => {
                send(
                    reply,
                    crate::log_store::clear_log_records(&mut self.connection).await,
                );
            }
        }
    }
}
