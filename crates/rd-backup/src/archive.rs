//! Writing and opening the sealed archive (RD-160-01).
//!
//! Both halves are blocking I/O and run on the blocking pool; the async service only waits for
//! them. Writing is the only path that produces an archive, and it takes a [`BackupKey`].

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::crypto::BackupKey;
use crate::manifest::{MANIFEST_NAME, MAX_MANIFEST_BYTES, Manifest, is_safe_member_name};
use crate::stream::{OpeningReader, SealingWriter};

/// What was written: the sealed archive's size and SHA-256 as it lies on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveSummary {
    pub size_bytes: u64,
    pub sha256: String,
}

/// Counts and hashes what passes through, so the archive's digest needs no second read.
struct Hashing<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(data)?;
        self.hasher.update(&data[..count]);
        self.written += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes `manifest` and every part it names, read from `staging`, sealed under `key`, to
/// `out`, which must not exist yet.
///
/// # Errors
///
/// When a part cannot be read, a member name is unsafe, or the archive cannot be written.
pub fn write_archive(
    staging: &Path,
    manifest: &Manifest,
    key: &BackupKey,
    out: &Path,
) -> Result<ArchiveSummary> {
    let file = File::create_new(out)
        .with_context(|| format!("create backup archive {}", out.display()))?;
    let hashing = Hashing {
        inner: BufWriter::new(file),
        hasher: Sha256::new(),
        written: 0,
    };
    let sealing = SealingWriter::new(hashing, key).context("start the sealed stream")?;
    let mut builder = tar::Builder::new(sealing);
    builder.mode(tar::HeaderMode::Deterministic);

    let manifest_bytes = serde_json::to_vec_pretty(manifest).context("encode the manifest")?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o600);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, MANIFEST_NAME, manifest_bytes.as_slice())
        .context("write the manifest")?;
    for part in &manifest.parts {
        anyhow::ensure!(
            is_safe_member_name(&part.name) && part.name != MANIFEST_NAME,
            "unsafe backup member name {}",
            part.name
        );
        let mut source = File::open(staging.join(&part.name))
            .with_context(|| format!("open backup part {}", part.name))?;
        builder
            .append_file(&part.name, &mut source)
            .with_context(|| format!("write backup part {}", part.name))?;
    }
    let sealing = builder.into_inner().context("finish the tar stream")?;
    let mut hashing = sealing.finish().context("seal the final chunk")?;
    hashing.flush()?;
    let Hashing {
        inner,
        hasher,
        written,
    } = hashing;
    let file = inner
        .into_inner()
        .map_err(|error| anyhow::anyhow!("flush backup archive: {}", error.error()))?;
    file.sync_all().context("sync backup archive")?;
    Ok(ArchiveSummary {
        size_bytes: written,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// Opens `archive` into `into`, checking every member against the manifest: its name, its
/// size and its SHA-256. A member the manifest does not name, a missing one, or one that
/// differs fails the whole open. Returns the manifest.
///
/// # Errors
///
/// When the key does not open the archive, it is damaged, or a member does not match.
pub fn extract_archive(archive: &Path, key: &BackupKey, into: &Path) -> Result<Manifest> {
    read_members(archive, key, |name, entry| {
        let target = into.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out =
            File::create_new(&target).with_context(|| format!("create {}", target.display()))?;
        let digest = copy_digest(entry, &mut out)?;
        out.sync_all()?;
        Ok(digest)
    })
}

/// Reads `archive` through to its end the way [`extract_archive`] does and writes nothing: every
/// chunk's tag, the final chunk's flag, and every member against the manifest (RD-160-02).
/// Returns the manifest.
///
/// This is what a verification of an archive at a destination runs. A changed byte fails its
/// chunk, a truncated archive fails its last chunk, and a member that does not match its
/// manifest entry — or one that is missing or extra — fails the check.
///
/// # Errors
///
/// When the key does not open the archive, it is damaged, or a member does not match.
pub fn verify_archive(archive: &Path, key: &BackupKey) -> Result<Manifest> {
    read_members(archive, key, |_, entry| copy_digest(entry, &mut io::sink()))
}

/// Copies a member to `out` and returns its size and SHA-256.
fn copy_digest(entry: &mut dyn Read, out: &mut dyn Write) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = entry.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        out.write_all(&buffer[..count])?;
        size += count as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

/// Opens the sealed stream, reads the manifest, hands every member to `member` — which
/// returns the size and SHA-256 it read — and checks each against the manifest, then that none
/// is missing. Reading to the end of the tar stream is what makes the stream check its final
/// chunk.
fn read_members(
    archive: &Path,
    key: &BackupKey,
    mut member: impl FnMut(&str, &mut dyn Read) -> Result<(u64, String)>,
) -> Result<Manifest> {
    let file = File::open(archive)
        .with_context(|| format!("open backup archive {}", archive.display()))?;
    let reader = OpeningReader::new(file, key).context("open the sealed stream")?;
    let mut tar = tar::Archive::new(reader);
    let mut entries = tar.entries().context("read the archive")?;

    let mut first = entries
        .next()
        .context("the archive is empty")?
        .context("read the manifest")?;
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
    drop(first);
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("parse the manifest")?;
    anyhow::ensure!(
        manifest.is_supported(),
        "unsupported backup format {} version {}",
        manifest.format,
        manifest.format_version
    );

    let mut seen = BTreeSet::new();
    for entry in entries {
        let mut entry = entry.context("read an archive member")?;
        let name = entry
            .path()?
            .to_str()
            .map(str::to_owned)
            .context("an archive member name is not UTF-8")?;
        anyhow::ensure!(
            entry.header().entry_type() == tar::EntryType::Regular,
            "archive member {name} is not a plain file"
        );
        let part = manifest
            .part(&name)
            .filter(|_| is_safe_member_name(&name))
            .with_context(|| format!("archive member {name} is not in the manifest"))?;
        anyhow::ensure!(
            seen.insert(name.clone()),
            "archive member {name} appears twice"
        );
        let (size, sha256) = member(&name, &mut entry)?;
        anyhow::ensure!(
            size == part.size && sha256 == part.sha256,
            "archive member {name} does not match its manifest entry"
        );
    }
    for part in &manifest.parts {
        anyhow::ensure!(
            seen.contains(&part.name),
            "archive member {} is missing",
            part.name
        );
    }
    // The tar stream ends before the sealed stream does; reading on to the end is what opens
    // the final chunk, so a cut or an appended tail cannot hide behind the end-of-archive mark.
    io::copy(&mut tar.into_inner(), &mut io::sink()).context("read the archive to its end")?;
    Ok(manifest)
}

/// The SHA-256 and size of one file, in 64 KiB reads.
///
/// # Errors
///
/// When the file cannot be read.
pub fn digest_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}
