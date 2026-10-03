//! The writer half of the archive passwords in the vault (RD-190-04); the steps and why they
//! come in this order are in `crate::archive_password`.

use super::{Writer, send};
use crate::{archive_password, commands::WriterCommand};

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_archive_passwords(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::ReserveArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::reserve(&mut self.connection, &references).await,
                );
            }
            WriterCommand::ReleaseArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::release(&mut self.connection, &references).await,
                );
            }
            WriterCommand::AdoptArchivePasswords {
                table,
                entries,
                reply,
            } => {
                send(
                    reply,
                    archive_password::adopt(&mut self.connection, table, &entries).await,
                );
            }
            WriterCommand::ForgetArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::forget(&mut self.connection, &references).await,
                );
            }
            // Routed here by `Writer::run` only for the variants above; see `maintenance.rs`
            // for why the rest is dropped rather than a panic.
            _ => {}
        }
    }
}
