//! Opening an archive for a restore (RD-160-03): the key from the passphrase, the check that it
//! opens the archive at all, the read-only pass a preview makes, and the unpack a test restore
//! and a restore start from.
//!
//! The key always comes from the passphrase the person types and the salt in the archive's
//! header, never from the key the service keeps for its scheduled runs (owner's decision,
//! 2026-09-28): a restore proves that somebody knows the passphrase, not that they hold a
//! session.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::RestoreError;
use crate::archive;
use crate::crypto::BackupKey;
use crate::manifest::{
    MANIFEST_NAME, MAX_MANIFEST_BYTES, Manifest, ManifestPart, is_safe_member_name,
};
use crate::stream::{self, OpeningReader};

/// The largest part a preview keeps in memory; the settings bundle and the two JSON lists are
/// far below it, and anything bigger is only hashed.
pub const MAX_KEPT_PART_BYTES: u64 = 64 << 20;

/// Derives the archive's key from `passphrase` and checks that it opens the first chunk.
///
/// # Errors
///
/// `backup.restore_not_archive` when the file is no archive of this build,
/// `backup.restore_passphrase_wrong` when the key does not open it — a damaged first chunk
/// looks the same, which the message says.
pub async fn key_for(archive: &Path, passphrase: &str) -> Result<BackupKey, RestoreError> {
    let header = {
        let archive = archive.to_path_buf();
        tokio::task::spawn_blocking(move || stream::read_header(&archive))
            .await
            .map_err(|error| RestoreError::new("backup.restore_failed", error))?
            .map_err(|error| RestoreError::new("backup.restore_not_archive", error))?
    };
    let key = BackupKey::derive(passphrase, header.salt)
        .await
        .map_err(|error| RestoreError::new("backup.restore_failed", error))?;
    let archive = archive.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = File::open(&archive)?;
        let mut reader = OpeningReader::new(file, &key)?;
        let mut first = [0_u8; 1];
        reader.read_exact(&mut first)?;
        Ok::<_, std::io::Error>(key)
    })
    .await
    .map_err(|error| RestoreError::new("backup.restore_failed", error))?
    .map_err(|error| RestoreError::new("backup.restore_passphrase_wrong", error))
}

/// What a read-only pass over an archive found: the manifest, and the parts it kept.
#[derive(Debug)]
pub struct ArchiveContents {
    pub manifest: Manifest,
    /// The parts `keep` asked for, by member name.
    pub kept: BTreeMap<String, Vec<u8>>,
}

/// Reads the whole archive without writing anything: every member is checked against the
/// manifest exactly as [`archive::extract_archive`] checks it, and the parts `keep` names are
/// held in memory. What a preview is built from — it changes no state.
///
/// # Errors
///
/// `backup.restore_damaged` when a member is unknown, missing, doubled or does not match.
pub async fn read_archive(
    archive: &Path,
    key: BackupKey,
    keep: fn(&ManifestPart) -> bool,
) -> Result<ArchiveContents, RestoreError> {
    let archive = archive.to_path_buf();
    tokio::task::spawn_blocking(move || read_blocking(&archive, &key, keep))
        .await
        .map_err(|error| RestoreError::new("backup.restore_failed", error))?
        .map_err(|error| RestoreError::new("backup.restore_damaged", format!("{error:#}")))
}

fn read_blocking(
    archive: &Path,
    key: &BackupKey,
    keep: fn(&ManifestPart) -> bool,
) -> anyhow::Result<ArchiveContents> {
    let reader = OpeningReader::new(File::open(archive)?, key)?;
    let mut tar = tar::Archive::new(reader);
    let mut entries = tar.entries()?;
    let mut first = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("the archive is empty"))??;
    anyhow::ensure!(
        first.path()?.to_str() == Some(MANIFEST_NAME),
        "the archive does not start with its manifest"
    );
    anyhow::ensure!(
        first.size() <= MAX_MANIFEST_BYTES,
        "the manifest is too large"
    );
    let mut manifest_bytes = Vec::new();
    first.read_to_end(&mut manifest_bytes)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
    anyhow::ensure!(
        manifest.is_supported(),
        "unsupported backup format {} version {}",
        manifest.format,
        manifest.format_version
    );

    let mut kept = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    for entry in entries {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("an archive member name is not UTF-8"))?;
        anyhow::ensure!(
            entry.header().entry_type() == tar::EntryType::Regular,
            "archive member {name} is not a plain file"
        );
        let part = manifest
            .part(&name)
            .filter(|_| is_safe_member_name(&name))
            .ok_or_else(|| anyhow::anyhow!("archive member {name} is not in the manifest"))?;
        anyhow::ensure!(
            seen.insert(name.clone()),
            "archive member {name} appears twice"
        );
        let keeping = keep(part) && part.size <= MAX_KEPT_PART_BYTES;
        let mut held = Vec::new();
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        loop {
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            size += count as u64;
            anyhow::ensure!(
                size <= part.size,
                "archive member {name} is longer than stated"
            );
            if keeping {
                held.extend_from_slice(&buffer[..count]);
            }
        }
        anyhow::ensure!(
            size == part.size && hex::encode(hasher.finalize()) == part.sha256,
            "archive member {name} does not match its manifest entry"
        );
        if keeping {
            kept.insert(name, held);
        }
    }
    for part in &manifest.parts {
        anyhow::ensure!(
            seen.contains(&part.name),
            "archive member {} is missing",
            part.name
        );
    }
    Ok(ArchiveContents { manifest, kept })
}

/// Unpacks and checks the whole archive into `into`, a folder that must not exist yet.
///
/// # Errors
///
/// `backup.restore_damaged` when the archive does not match its manifest.
pub async fn unpack(archive: &Path, key: BackupKey, into: &Path) -> Result<Manifest, RestoreError> {
    let archive = archive.to_path_buf();
    let into = into.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<Manifest> {
        std::fs::create_dir(&into)?;
        archive::extract_archive(&archive, &key, &into)
    })
    .await
    .map_err(|error| RestoreError::new("backup.restore_failed", error))?
    .map_err(|error| RestoreError::new("backup.restore_damaged", format!("{error:#}")))
}
