//! SFV (Simple File Verification) parsing and CRC32 verification.
//!
//! SFV is a de-facto format, not a standard: a plain text index of `name CRC32` lines
//! that releases ship next to their archive volumes. Anything that does not parse as
//! such a line is ignored rather than treated as an error, because real indexes carry
//! tool banners, blank lines and `;` comments.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rd_core::ChecksumAlgorithm;
use rd_files::compute_checksum;

use crate::{
    archive::safe_relative,
    progress::{ExtractProgress, ProgressSender, percent_of, report},
};

/// One `name CRC32` line of an SFV index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SfvEntry {
    pub name: String,
    /// Lower-case and eight digits wide — the form `compute_checksum` returns for CRC32.
    pub crc32: String,
}

/// Outcome of verifying one SFV index.
#[derive(Clone, Debug, Default)]
pub struct SfvReport {
    /// Files that were opened and hashed.
    pub checked: usize,
    /// Names whose CRC32 differed from the index.
    pub mismatched: Vec<String>,
    /// Names listed in the index but absent from the package folder.
    pub missing: Vec<String>,
    /// Entries whose path would leave the package folder; never opened.
    pub skipped: usize,
}

impl SfvReport {
    /// Everything listed was present and matched.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.mismatched.is_empty() && self.missing.is_empty()
    }
}

/// Whether a path carries the `.sfv` extension, in any casing.
#[must_use]
pub fn is_sfv(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sfv"))
}

/// Parses an SFV index; unparsable lines are dropped.
#[must_use]
pub fn parse_sfv(contents: &str) -> Vec<SfvEntry> {
    contents.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<SfvEntry> {
    let line = line.trim_start_matches('\u{feff}').trim();
    if line.is_empty() || line.starts_with(';') {
        return None;
    }
    // File names may contain spaces, so the checksum is the last token, not the second one.
    let split = line.rfind(char::is_whitespace)?;
    let (name, crc32) = line.split_at(split);
    let (name, crc32) = (name.trim(), crc32.trim());
    if name.is_empty() || crc32.len() != 8 || !crc32.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(SfvEntry {
        name: name.to_owned(),
        crc32: crc32.to_ascii_lowercase(),
    })
}

/// Verifies every file an SFV index lists, relative to `directory`.
///
/// Reading the index never fails the check on encoding: SFV files predate UTF-8 and are
/// frequently Latin-1, and a lossy decode keeps the ASCII names and checksums intact.
pub async fn verify_sfv(
    index: PathBuf,
    directory: PathBuf,
    progress: Option<&ProgressSender>,
) -> Result<SfvReport> {
    let bytes = tokio::fs::read(&index)
        .await
        .with_context(|| format!("read SFV index {}", index.display()))?;
    let entries = parse_sfv(&String::from_utf8_lossy(&bytes));

    // First pass resolves paths and sizes so the second one can report a byte percent.
    let mut report = SfvReport::default();
    let mut pending: Vec<(SfvEntry, PathBuf, u64)> = Vec::new();
    let mut total_bytes = 0_u64;
    for entry in entries {
        let Ok(relative) = safe_relative(&entry.name) else {
            report.skipped += 1;
            continue;
        };
        let path = directory.join(relative);
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {
                total_bytes += metadata.len();
                pending.push((entry, path, metadata.len()));
            }
            _ => report.missing.push(entry.name),
        }
    }

    let mut done_bytes = 0_u64;
    for (entry, path, size) in pending {
        report_sample(progress, done_bytes, total_bytes, Some(entry.name.clone()));
        let computed = compute_checksum(&path, ChecksumAlgorithm::Crc32)
            .await
            .with_context(|| format!("checksum {}", path.display()))?;
        report.checked += 1;
        if computed.value != entry.crc32 {
            report.mismatched.push(entry.name);
        }
        done_bytes += size;
    }
    report_sample(progress, done_bytes, total_bytes, None);
    Ok(report)
}

fn report_sample(
    progress: Option<&ProgressSender>,
    done_bytes: u64,
    total_bytes: u64,
    current: Option<String>,
) {
    report(
        progress,
        ExtractProgress {
            done_bytes,
            total_bytes: Some(total_bytes),
            percent: percent_of(done_bytes, Some(total_bytes)),
            current,
        },
    );
}
