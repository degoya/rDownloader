//! The writer half of the archive passwords in the vault (RD-190-04); the steps and why they
//! come in this order are in `crate::archive_password`.

use super::{Writer, send};
use crate::{archive_password, commands::ArchivePasswordsCommand};

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_archive_passwords(&mut self, command: ArchivePasswordsCommand) {
        match command {
            ArchivePasswordsCommand::ReserveArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::reserve(&mut self.connection, &references).await,
                );
            }
            ArchivePasswordsCommand::ReleaseArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::release(&mut self.connection, &references).await,
                );
            }
            ArchivePasswordsCommand::AdoptArchivePasswords {
                table,
                entries,
                reply,
            } => {
                send(
                    reply,
                    archive_password::adopt(&mut self.connection, table, &entries).await,
                );
            }
            ArchivePasswordsCommand::ForgetArchivePasswords { references, reply } => {
                send(
                    reply,
                    archive_password::forget(&mut self.connection, &references).await,
                );
            }
        }
    }
}
