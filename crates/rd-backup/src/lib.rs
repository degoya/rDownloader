//! Encrypted full backups of an installation (RD-160-01).
//!
//! One archive holds everything an installation needs besides its downloaded payload: the
//! settings bundle with its credentials, a consistent copy of the database, the torrent session
//! and the stored `.torrent` files, the plugin trust, and the list of unfinished transfers. The
//! archive is a tar stream inside an authenticated encryption stream ([`stream`]), with a
//! manifest ([`manifest`]) that names every part with its size and SHA-256.
//!
//! **There is no unencrypted full backup.** The only function that writes an archive,
//! [`archive::write_archive`], takes a [`BackupKey`]; nothing in this crate can produce the tar
//! stream on its own. The key is derived once from a passphrase ([`crypto`]) and kept in the
//! secret store by the service; the passphrase is stored nowhere.
//!
//! Where the archive goes is a [`BackupDestination`]: a local folder (a NAS mount included),
//! a folder of an object storage bucket or an rclone remote ([`remote`], RD-160-02), each
//! receiving its own copy ([`deliver`]), recorded in the ledger ([`ledger`]), and keeping as many
//! archives as its [`retention`] says. [`verify`] checks an archive where it lies. When a run happens is [`schedule`], the
//! cron arithmetic the subscriptions use, read in an IANA zone. Getting an installation back from an archive — preview,
//! test restore, path remap, cutover — is [`restore`] (RD-160-03). The verified database copy and
//! archive the updater asks for before it switches versions are [`pre_update`] (RD-180-03).

#![warn(unreachable_pub)]

pub mod archive;
mod create;
pub mod crypto;
pub mod deliver;
pub mod destination;
pub mod ledger;
pub mod manifest;
pub mod pre_update;
pub mod remote;
pub mod restore;
pub mod retention;
pub mod schedule;
pub mod stream;
pub mod verify;

pub use create::{
    ARCHIVE_PREFIX, BackupError, BackupSources, CreatedBackup, STAGING_DIR, SealedBackup,
    archive_name, create_backup, seal_backup, staging_root, sweep_staging,
};
pub use crypto::{BackupKey, MIN_PASSPHRASE_CHARS};
pub use deliver::{Delivery, RetryPolicy, deliver, deliver_all};
pub use destination::{
    BackupDestination, DestinationError, ListedArchive, LocalFolder, StoredBackup, is_archive_name,
};
pub use manifest::{FORMAT_VERSION, Manifest, ManifestPart, PartKind};
pub use remote::{DestinationConfig, DestinationContext};
pub use retention::{RecordedArchive, RetentionPlan, RetentionPolicy, is_own_archive};

/// The file extension of an archive.
pub const ARCHIVE_EXTENSION: &str = "rdbackup";
