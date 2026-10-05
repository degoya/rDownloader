//! The commands of `writer/full_backup.rs`.

use chrono::{DateTime, Utc};

use super::Reply;

/// The commands `Writer::handle_full_backup` applies.
pub(crate) enum FullBackupCommand {
    SaveBackupConfig {
        update: Box<crate::BackupConfigUpdate>,
        reply: Reply<()>,
    },
    SetBackupKey {
        key: crate::BackupKeyRecord,
        reply: Reply<Option<String>>,
    },
    ArmBackup {
        next_run_at: Option<DateTime<Utc>>,
        reply: Reply<()>,
    },
    BeginBackupRun {
        run: crate::NewBackupRun,
        reply: Reply<bool>,
    },
    FinishBackupRun {
        id: String,
        outcome: Box<crate::BackupRunOutcome>,
        reply: Reply<()>,
    },
    InterruptBackupRuns {
        reply: Reply<u64>,
    },
}

/// The commands `Writer::handle_backup_ledger` applies.
pub(crate) enum BackupLedgerCommand {
    /// The full backup's destinations, ledger and verifications (RD-160-02).
    CreateBackupDestination {
        destination: Box<crate::NewBackupDestination>,
        reply: Reply<String>,
    },
    UpdateBackupDestination {
        id: String,
        destination: Box<crate::NewBackupDestination>,
        reply: Reply<bool>,
    },
    DeleteBackupDestination {
        id: String,
        reply: Reply<bool>,
    },
    RecordBackupArchive {
        archive: Box<crate::NewBackupArchive>,
        reply: Reply<String>,
    },
    ForgetBackupArchives {
        ids: Vec<String>,
        reply: Reply<u64>,
    },
    BeginBackupRunDestinations {
        run_id: String,
        destinations: Vec<(String, String, String)>,
        reply: Reply<()>,
    },
    FinishBackupRunDestination {
        end: Box<crate::BackupRunDestinationEnd>,
        reply: Reply<()>,
    },
    BeginBackupVerification {
        verification: Box<crate::BackupVerification>,
        reply: Reply<()>,
    },
    FinishBackupVerification {
        id: String,
        outcome: crate::BackupVerificationOutcome,
        reply: Reply<()>,
    },
    ArmBackupVerify {
        next_run_at: Option<DateTime<Utc>>,
        reply: Reply<()>,
    },
}
