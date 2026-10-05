//! The writer half of `history_store`: retention, clearing and the SABnzbd view. The entries
//! themselves are written by the package's own state change (`writer_jobs.rs`).

use super::{Writer, send};
use crate::commands::HistoryCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_history(&mut self, command: HistoryCommand) {
        match command {
            HistoryCommand::PruneHistory {
                max_entries,
                older_than,
                reply,
            } => {
                let result =
                    crate::history_store::prune(&mut self.connection, max_entries, older_than)
                        .await;
                send(reply, result);
            }
            HistoryCommand::ClearHistory { reply } => {
                send(
                    reply,
                    crate::history_store::clear(&mut self.connection).await,
                );
            }
            HistoryCommand::HideHistoryFromCompat { package_ids, reply } => {
                let result =
                    crate::history_store::hide_from_compat(&mut self.connection, package_ids).await;
                send(reply, result);
            }
        }
    }
}
