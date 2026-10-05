//! The writer half of `full_backup_store` (RD-160-01) and `backup_ledger_store` (RD-160-02).
//!
//! None of these raise an event: the backup page reads its state when it opens and after it
//! acts, and nothing else in the process re-reads anything when a backup row changes.

use super::{Writer, send};
use crate::commands::{BackupLedgerCommand, FullBackupCommand};

impl Writer {
    /// Applies the full backup commands.
    pub(super) async fn handle_full_backup(&mut self, command: FullBackupCommand) {
        match command {
            FullBackupCommand::SaveBackupConfig { update, reply } => {
                let result =
                    crate::full_backup_store::save_config(&mut self.connection, *update).await;
                send(reply, result);
            }
            FullBackupCommand::SetBackupKey { key, reply } => {
                send(
                    reply,
                    crate::full_backup_store::set_key(&mut self.connection, key).await,
                );
            }
            FullBackupCommand::ArmBackup { next_run_at, reply } => {
                send(
                    reply,
                    crate::full_backup_store::arm(&mut self.connection, next_run_at).await,
                );
            }
            FullBackupCommand::BeginBackupRun { run, reply } => {
                send(
                    reply,
                    crate::full_backup_store::begin_run(&mut self.connection, run).await,
                );
            }
            FullBackupCommand::FinishBackupRun { id, outcome, reply } => {
                let result =
                    crate::full_backup_store::finish_run(&mut self.connection, &id, *outcome).await;
                send(reply, result);
            }
            FullBackupCommand::InterruptBackupRuns { reply } => {
                send(
                    reply,
                    crate::full_backup_store::interrupt_runs(&mut self.connection).await,
                );
            }
        }
    }

    /// Applies the destination, ledger and verification commands.
    pub(super) async fn handle_backup_ledger(&mut self, command: BackupLedgerCommand) {
        use crate::backup_ledger_store as ledger;
        let connection = &mut self.connection;
        match command {
            BackupLedgerCommand::CreateBackupDestination { destination, reply } => {
                send(
                    reply,
                    ledger::create_destination(connection, *destination).await,
                );
            }
            BackupLedgerCommand::UpdateBackupDestination {
                id,
                destination,
                reply,
            } => {
                send(
                    reply,
                    ledger::update_destination(connection, &id, *destination).await,
                );
            }
            BackupLedgerCommand::DeleteBackupDestination { id, reply } => {
                send(reply, ledger::delete_destination(connection, &id).await);
            }
            BackupLedgerCommand::RecordBackupArchive { archive, reply } => {
                send(reply, ledger::record_archive(connection, *archive).await);
            }
            BackupLedgerCommand::ForgetBackupArchives { ids, reply } => {
                send(reply, ledger::forget_archives(connection, ids).await);
            }
            BackupLedgerCommand::BeginBackupRunDestinations {
                run_id,
                destinations,
                reply,
            } => {
                send(
                    reply,
                    ledger::begin_run_destinations(connection, &run_id, destinations).await,
                );
            }
            BackupLedgerCommand::FinishBackupRunDestination { end, reply } => {
                send(
                    reply,
                    ledger::finish_run_destination(connection, *end).await,
                );
            }
            BackupLedgerCommand::BeginBackupVerification {
                verification,
                reply,
            } => {
                send(
                    reply,
                    ledger::begin_verification(connection, *verification).await,
                );
            }
            BackupLedgerCommand::FinishBackupVerification { id, outcome, reply } => {
                send(
                    reply,
                    ledger::finish_verification(connection, &id, outcome).await,
                );
            }
            BackupLedgerCommand::ArmBackupVerify { next_run_at, reply } => {
                send(reply, ledger::arm_verify(connection, next_run_at).await);
            }
        }
    }
}
