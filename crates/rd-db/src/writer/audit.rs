//! The writer half of `audit_store`: the append and the bounded prune. There is no update.

use super::{Writer, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_audit(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::AppendAuditRecords { records, reply } => {
                let result =
                    crate::audit_store::append_audit_records(&mut self.connection, &records).await;
                send(reply, result);
            }
            WriterCommand::PruneAuditRecords {
                max_records,
                older_than,
                batch,
                reply,
            } => {
                let result = crate::audit_store::prune_audit_records(
                    &mut self.connection,
                    max_records,
                    older_than,
                    batch,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::ClearAuditRecords { record, reply } => {
                send(
                    reply,
                    crate::audit_store::clear_audit_records(&mut self.connection, *record).await,
                );
            }
            // See `handle_logs` for why this drops rather than panics.
            _ => {}
        }
    }
}
