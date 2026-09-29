//! The writer half of `full_backup_store` (RD-160-01) and `backup_ledger_store` (RD-160-02).
//!
//! None of these raise an event: the backup page reads its state when it opens and after it
//! acts, and nothing else in the process re-reads anything when a backup row changes.

use super::{Writer, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the full backup commands.
    pub(super) async fn handle_full_backup(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::SaveBackupConfig { update, reply } => {
                let result =
                    crate::full_backup_store::save_config(&mut self.connection, *update).await;
                send(reply, result);
            }
            WriterCommand::SetBackupKey { key, reply } => {
                send(
                    reply,
                    crate::full_backup_store::set_key(&mut self.connection, key).await,
                );
            }
            WriterCommand::ArmBackup { next_run_at, reply } => {
                send(
                    reply,
                    crate::full_backup_store::arm(&mut self.connection, next_run_at).await,
                );
            }
            WriterCommand::BeginBackupRun { run, reply } => {
                send(
                    reply,
                    crate::full_backup_store::begin_run(&mut self.connection, run).await,
                );
            }
            WriterCommand::FinishBackupRun { id, outcome, reply } => {
                let result =
                    crate::full_backup_store::finish_run(&mut self.connection, &id, *outcome).await;
                send(reply, result);
            }
            WriterCommand::InterruptBackupRuns { reply } => {
                send(
                    reply,
                    crate::full_backup_store::interrupt_runs(&mut self.connection).await,
                );
            }
            // Routed here by `Writer::run` only for the variants above; see `handle_plugins`.
            _ => {}
        }
    }

    /// Applies the destination, ledger and verification commands.
    pub(super) async fn handle_backup_ledger(&mut self, command: WriterCommand) {
        use crate::backup_ledger_store as ledger;
        let connection = &mut self.connection;
        match command {
            WriterCommand::CreateBackupDestination { destination, reply } => {
                send(
                    reply,
                    ledger::create_destination(connection, *destination).await,
                );
            }
            WriterCommand::UpdateBackupDestination {
                id,
                destination,
                reply,
            } => {
                send(
                    reply,
                    ledger::update_destination(connection, &id, *destination).await,
                );
            }
            WriterCommand::DeleteBackupDestination { id, reply } => {
                send(reply, ledger::delete_destination(connection, &id).await);
            }
            WriterCommand::RecordBackupArchive { archive, reply } => {
                send(reply, ledger::record_archive(connection, *archive).await);
            }
            WriterCommand::ForgetBackupArchives { ids, reply } => {
                send(reply, ledger::forget_archives(connection, ids).await);
            }
            WriterCommand::BeginBackupRunDestinations {
                run_id,
                destinations,
                reply,
            } => {
                send(
                    reply,
                    ledger::begin_run_destinations(connection, &run_id, destinations).await,
                );
            }
            WriterCommand::FinishBackupRunDestination { end, reply } => {
                send(
                    reply,
                    ledger::finish_run_destination(connection, *end).await,
                );
            }
            WriterCommand::BeginBackupVerification {
                verification,
                reply,
            } => {
                send(
                    reply,
                    ledger::begin_verification(connection, *verification).await,
                );
            }
            WriterCommand::FinishBackupVerification { id, outcome, reply } => {
                send(
                    reply,
                    ledger::finish_verification(connection, &id, outcome).await,
                );
            }
            WriterCommand::ArmBackupVerify { next_run_at, reply } => {
                send(reply, ledger::arm_verify(connection, next_run_at).await);
            }
            // Routed here by `Writer::run` only for the variants above.
            _ => {}
        }
    }
}
