//! The commands of `writer/maintenance.rs`.

use super::Reply;

/// The commands `Writer::handle_maintenance` applies.
// `ReplaceConfig` outweighs the rest of the area; every message is as large as the largest
// command, as it was with one flat enum.
#[allow(clippy::large_enum_variant)]
pub(crate) enum MaintenanceCommand {
    ReplaceConfig {
        replacement: crate::backup_store::ConfigReplacement,
        reply: Reply<Vec<String>>,
    },
    RecoverInterrupted {
        reply: Reply<u64>,
    },
    CheckpointWal {
        reply: Reply<()>,
    },
    SetSetting {
        key: String,
        value: serde_json::Value,
        reply: Reply<()>,
    },
    /// Writes a setting only if the key holds nothing yet; replies whether it did.
    InsertSettingIfAbsent {
        key: String,
        value: serde_json::Value,
        reply: Reply<bool>,
    },
    PurgeOldEvents {
        reply: Reply<u64>,
    },
    /// One bounded pass of the transfer-statistics retention sweep (RD-110-01).
    PruneTransferStats {
        retention: crate::StatsRetention,
        reply: Reply<crate::StatsPruneReport>,
    },
    /// Empties both statistics tables and reports how many rows went.
    ClearTransferStats {
        reply: Reply<u64>,
    },
    /// Rewrites the database file without its free pages (RD-190-04: after the takeover of
    /// the archive passwords, so no page the plain columns once used survives in the file).
    Vacuum {
        reply: Reply<()>,
    },
    /// Writes a consistent copy of the whole database to `path` (RD-160-01).
    VacuumInto {
        path: std::path::PathBuf,
        reply: Reply<()>,
    },
}
