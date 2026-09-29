//! Database facade methods for full backups (RD-160-01), their destinations, the ledger of
//! written archives and the verifications (RD-160-02).

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database,
    backup_ledger_store::{
        self, BackupArchive, BackupRunDestinationEnd, BackupVerification,
        BackupVerificationOutcome, NewBackupArchive,
    },
    commands::WriterCommand,
    full_backup_store::{
        self, BackupConfig, BackupConfigUpdate, BackupDestinationRecord, BackupKeyRecord,
        BackupRun, BackupRunOutcome, NewBackupDestination, NewBackupRun,
    },
    writer,
};

impl Database {
    /// Writes a consistent copy of the whole database to `path`, which must not exist yet.
    ///
    /// The copy is taken in the writer's order: it holds every mutation sent before this call
    /// and none sent after it. Writes wait while it is taken; see `crate::snapshot`.
    pub async fn snapshot_into(&self, path: &Path) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::VacuumInto {
            path: path.to_path_buf(),
            reply,
        })
        .await
    }

    /// The backup configuration, with its destination and the reference to its key.
    pub async fn backup_config(&self) -> Result<BackupConfig> {
        full_backup_store::config(&self.readers).await
    }

    /// Saves the schedule and replaces the destination.
    pub async fn save_backup_config(&self, update: BackupConfigUpdate) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SaveBackupConfig {
            update: Box::new(update),
            reply,
        })
        .await
    }

    /// Replaces the key reference; returns the one it replaced.
    pub async fn set_backup_key(&self, key: BackupKeyRecord) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| WriterCommand::SetBackupKey {
            key,
            reply,
        })
        .await
    }

    /// Sets when the schedule is next due.
    pub async fn arm_backup(&self, next_run_at: Option<DateTime<Utc>>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ArmBackup {
            next_run_at,
            reply,
        })
        .await
    }

    /// Records the start of a run; `false` when another one is still running.
    pub async fn begin_backup_run(&self, run: NewBackupRun) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::BeginBackupRun {
            run,
            reply,
        })
        .await
    }

    /// Records how a run ended.
    pub async fn finish_backup_run(&self, id: String, outcome: BackupRunOutcome) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::FinishBackupRun {
            id,
            outcome: Box::new(outcome),
            reply,
        })
        .await
    }

    /// Marks every run still `running` as interrupted; for the start of the process only.
    pub async fn interrupt_backup_runs(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::InterruptBackupRuns {
            reply,
        })
        .await
    }

    /// The newest runs first.
    pub async fn backup_runs(&self, limit: u32) -> Result<Vec<BackupRun>> {
        full_backup_store::runs(&self.readers, limit).await
    }

    /// One run by id.
    pub async fn backup_run(&self, id: &str) -> Result<Option<BackupRun>> {
        full_backup_store::run(&self.readers, id).await
    }

    /// Every destination, oldest first.
    pub async fn backup_destinations(&self) -> Result<Vec<BackupDestinationRecord>> {
        backup_ledger_store::destinations(&self.readers).await
    }

    /// One destination by id.
    pub async fn backup_destination(&self, id: &str) -> Result<Option<BackupDestinationRecord>> {
        backup_ledger_store::destination(&self.readers, id).await
    }

    /// Adds a destination; returns its id.
    pub async fn create_backup_destination(
        &self,
        destination: NewBackupDestination,
    ) -> Result<String> {
        writer::request(&self.writer, |reply| {
            WriterCommand::CreateBackupDestination {
                destination: Box::new(destination),
                reply,
            }
        })
        .await
    }

    /// Replaces a destination; `false` when there is none of that id.
    pub async fn update_backup_destination(
        &self,
        id: String,
        destination: NewBackupDestination,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateBackupDestination {
                id,
                destination: Box::new(destination),
                reply,
            }
        })
        .await
    }

    /// Removes a destination and forgets its archives; the archives stay where they are.
    pub async fn delete_backup_destination(&self, id: String) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteBackupDestination { id, reply }
        })
        .await
    }

    /// Records an archive placed at a destination; returns its ledger id.
    pub async fn record_backup_archive(&self, archive: NewBackupArchive) -> Result<String> {
        writer::request(&self.writer, |reply| WriterCommand::RecordBackupArchive {
            archive: Box::new(archive),
            reply,
        })
        .await
    }

    /// The ledger, newest first; of one destination or of all.
    pub async fn backup_archives(
        &self,
        destination_id: Option<&str>,
    ) -> Result<Vec<BackupArchive>> {
        backup_ledger_store::archives(&self.readers, destination_id).await
    }

    /// One archive of the ledger by id.
    pub async fn backup_archive(&self, id: &str) -> Result<Option<BackupArchive>> {
        backup_ledger_store::archive(&self.readers, id).await
    }

    /// Forgets archives retention removed.
    pub async fn forget_backup_archives(&self, ids: Vec<String>) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ForgetBackupArchives {
            ids,
            reply,
        })
        .await
    }

    /// Records the destinations a run delivers to, as `(id, kind, name)`.
    pub async fn begin_backup_run_destinations(
        &self,
        run_id: String,
        destinations: Vec<(String, String, String)>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::BeginBackupRunDestinations {
                run_id,
                destinations,
                reply,
            }
        })
        .await
    }

    /// Records how one destination of a run fared.
    pub async fn finish_backup_run_destination(&self, end: BackupRunDestinationEnd) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::FinishBackupRunDestination {
                end: Box::new(end),
                reply,
            }
        })
        .await
    }

    /// Records the start of a verification.
    pub async fn begin_backup_verification(&self, verification: BackupVerification) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::BeginBackupVerification {
                verification: Box::new(verification),
                reply,
            }
        })
        .await
    }

    /// Records how a verification ended, on its row and on the archive's.
    pub async fn finish_backup_verification(
        &self,
        id: String,
        outcome: BackupVerificationOutcome,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::FinishBackupVerification { id, outcome, reply }
        })
        .await
    }

    /// The newest verifications first.
    pub async fn backup_verifications(&self, limit: u32) -> Result<Vec<BackupVerification>> {
        backup_ledger_store::verifications(&self.readers, limit).await
    }

    /// Sets when the scheduled verification is next due.
    pub async fn arm_backup_verify(&self, next_run_at: Option<DateTime<Utc>>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ArmBackupVerify {
            next_run_at,
            reply,
        })
        .await
    }
}
