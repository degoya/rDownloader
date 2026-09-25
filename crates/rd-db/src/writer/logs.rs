//! The writer half of `log_store`: the batched append and the bounded prune.

use super::{Writer, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_logs(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::AppendLogRecords { records, reply } => {
                let result =
                    crate::log_store::append_log_records(&mut self.connection, &records).await;
                send(reply, result);
            }
            WriterCommand::PruneLogRecords {
                max_records,
                older_than,
                batch,
                reply,
            } => {
                let result = crate::log_store::prune_log_records(
                    &mut self.connection,
                    max_records,
                    older_than,
                    batch,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::ClearLogRecords { reply } => {
                send(
                    reply,
                    crate::log_store::clear_log_records(&mut self.connection).await,
                );
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
