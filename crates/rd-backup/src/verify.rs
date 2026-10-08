//! Checking an archive where it lies (RD-160-02).
//!
//! The archive comes back from its destination into the staging folder and is checked twice:
//!
//! 1. against the ledger — the size and the SHA-256 the sealed file had when this installation
//!    wrote it. A truncated, extended or changed file fails here, whatever key it is under;
//! 2. against itself — opened with the backup key and read to its end: every chunk's tag, the
//!    final chunk, every member against the manifest ([`archive::verify_archive`]). This needs
//!    the key the archive was sealed with; an archive from before the passphrase was replaced
//!    gets the first check only, and the outcome says so.
//!
//! The copy is removed afterwards, whatever the outcome.

use std::path::Path;

use crate::archive::{self, digest_file};
use crate::create::BackupError;
use crate::crypto::BackupKey;
use crate::destination::{BackupDestination, DestinationError};

/// Stable code of an archive the destination no longer has.
pub const VERIFY_MISSING: &str = "backup.verify_missing";
/// Stable code of an archive whose size or SHA-256 is not what was written.
pub const VERIFY_DIGEST_MISMATCH: &str = "backup.verify_digest_mismatch";
/// Stable code of an archive the key opens but whose content does not check out.
pub const VERIFY_DAMAGED: &str = "backup.verify_damaged";

/// What was written, as the ledger recorded it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedArchive {
    pub name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// A passed verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verified {
    /// Whether the content was opened and checked too, or only the digest (another key).
    pub content_checked: bool,
}

fn failure(code: &'static str, detail: impl Into<String>) -> BackupError {
    BackupError {
        code,
        detail: detail.into(),
    }
}

/// Fetches `expected` from `destination` into `scratch` (a folder, created when missing) and
/// checks it.
///
/// # Errors
///
/// With the check that failed as the code, or the destination's own when the fetch did.
pub async fn verify_at(
    destination: &dyn BackupDestination,
    expected: &ExpectedArchive,
    key: &BackupKey,
    scratch: &Path,
) -> Result<Verified, BackupError> {
    crate::private_folder(scratch)
        .await
        .map_err(|error| failure("backup.verify_failed", error.to_string()))?;
    let copy = scratch.join(format!("verify-{}", uuid::Uuid::now_v7()));
    let result = check(destination, expected, key, &copy).await;
    if let Err(error) = tokio::fs::remove_file(&copy).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "a verified archive copy could not be removed");
    }
    result
}

async fn check(
    destination: &dyn BackupDestination,
    expected: &ExpectedArchive,
    key: &BackupKey,
    copy: &Path,
) -> Result<Verified, BackupError> {
    destination
        .fetch(&expected.name, copy)
        .await
        .map_err(|error| match error {
            DestinationError::NotFound(name) => failure(
                VERIFY_MISSING,
                format!("{name} is no longer at {}", destination.describe()),
            ),
            other => failure(other.code(), other.to_string()),
        })?;
    let key = BackupKey::from_stored(key.key_bytes(), &key.salt())
        .map_err(|error| failure("backup.verify_failed", format!("{error:#}")))?;
    let path = copy.to_path_buf();
    let expected = expected.clone();
    tokio::task::spawn_blocking(move || {
        let (size, sha256) = digest_file(&path)
            .map_err(|error| failure("backup.verify_failed", error.to_string()))?;
        if size != expected.size_bytes || sha256 != expected.sha256 {
            return Err(failure(
                VERIFY_DIGEST_MISMATCH,
                format!(
                    "{} holds {size} bytes with SHA-256 {sha256}; {} bytes with {} were written",
                    expected.name, expected.size_bytes, expected.sha256
                ),
            ));
        }
        let header = crate::stream::read_header(&path)
            .map_err(|error| failure(VERIFY_DAMAGED, error.to_string()))?;
        if header.salt != key.salt() {
            return Ok(Verified {
                content_checked: false,
            });
        }
        archive::verify_archive(&path, &key)
            .map_err(|error| failure(VERIFY_DAMAGED, format!("{error:#}")))?;
        Ok(Verified {
            content_checked: true,
        })
    })
    .await
    .map_err(|error| failure("backup.verify_failed", error.to_string()))?
}
