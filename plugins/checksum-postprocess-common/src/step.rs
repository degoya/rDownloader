//! The component's run: verify every sidecar of one [`Algorithm`] the package carries.

use std::collections::BTreeSet;

use crate::{
    Checksum,
    exports::rdownloader::plugin::postprocess::{StepComplete, StepEnd, StepInput},
    rdownloader::plugin::{
        source,
        types::{Failure, FailureKind, LabelPart},
    },
    sidecar::{self, Algorithm},
};

/// Bytes read per call. Large enough to be worth the crossing, small enough that one read
/// cannot claim the whole response budget.
const CHUNK: u32 = 256 * 1024;

/// Longest sidecar read. A checksum file is a list of lines; anything larger is not one.
const MAX_SIDECAR: u64 = 4 * 1024 * 1024;

/// The step: every sidecar of `algorithm` in the package, each listed file hashed with `H`.
pub fn run<H: Checksum>(algorithm: &Algorithm, input: StepInput) -> StepEnd {
    let sidecars: Vec<&String> = input
        .files
        .iter()
        .filter(|name| algorithm.is_sidecar(name))
        .collect();
    // No sidecar is not a failure: most packages have none, and a step that reported one
    // would fail every package in a category it was switched on for.
    if sidecars.is_empty() {
        return StepEnd::Skipped;
    }
    // Everything the sidecars ask for, flattened and ordered, so a checkpoint is just
    // "how many of these are done" and resuming needs no bookkeeping of its own.
    let files: BTreeSet<&str> = input.files.iter().map(String::as_str).collect();
    // What the pipeline removed before this step: a sidecar listing it is not wrong for it.
    let removed: BTreeSet<&str> = input.removed.iter().map(String::as_str).collect();
    let mut wanted = Vec::new();
    let mut unchecked = Vec::new();
    for name in sidecars {
        let text = match read_text(algorithm, &input.handle, name) {
            Ok(text) => text,
            Err(failure) => return StepEnd::Failed(failure),
        };
        // Each entry is read from the sidecar's own folder (`Film/film.mkv`, RD-170-16).
        match algorithm.wanted(&files, &removed, name, &text) {
            Ok(plan) => {
                wanted.extend(plan.entries);
                unchecked.extend(
                    plan.unchecked
                        .into_iter()
                        .map(|file| format!("{file} (in {name})")),
                );
            }
            Err(problem) => return StepEnd::Failed(unverifiable(algorithm, name, problem)),
        }
    }
    // Every listed file was one post-processing had already unpacked and removed.
    if wanted.is_empty() {
        return StepEnd::Skipped;
    }
    let done = input
        .checkpoint
        .as_deref()
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0, |bytes| u32::from_le_bytes(bytes) as usize);
    let total = wanted.len();
    for (index, entry) in wanted.iter().enumerate().skip(done) {
        if source::should_stop() {
            return StepEnd::Stopped(u32_bytes(index));
        }
        match digest_of::<H>(&input.handle, &entry.file) {
            Ok(digest) if digest == entry.digest => {}
            Ok(digest) => {
                return StepEnd::Failed(mismatch(algorithm, &entry.file, &entry.digest, &digest));
            }
            Err(failure) => return StepEnd::Failed(failure),
        }
        source::progress((index + 1) as u64, Some(total as u64));
    }
    // A pass with files left unchecked says so on the step (RD-190-06).
    StepEnd::Complete(StepComplete {
        checkpoint: None,
        warnings: if unchecked.is_empty() {
            Vec::new()
        } else {
            vec![unchecked_warning(algorithm, &unchecked)]
        },
    })
}

/// Reads one file whole, for a sidecar small enough to hold in memory.
fn read_text(algorithm: &Algorithm, handle: &str, name: &str) -> Result<String, Failure> {
    let size = source::size_of(handle, name)?;
    if size > MAX_SIDECAR {
        return Err(refuse(
            algorithm,
            format!("{name} is too large to be a checksum file"),
        ));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let chunk = source::read_at(handle, name, bytes.len() as u64, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| refuse(algorithm, format!("{name} is not text")))
}

/// Streams one file through the hash, so a package larger than memory still verifies.
fn digest_of<H: Checksum>(handle: &str, name: &str) -> Result<String, Failure> {
    let size = source::size_of(handle, name)?;
    let mut hasher = H::new();
    let mut offset = 0_u64;
    while offset < size {
        let chunk = source::read_at(handle, name, offset, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        offset += chunk.len() as u64;
        hasher.update(&chunk);
    }
    Ok(plugin_common::encode::to_hex(&hasher.finish()))
}

fn u32_bytes(value: usize) -> Vec<u8> {
    u32::try_from(value)
        .unwrap_or(u32::MAX)
        .to_le_bytes()
        .to_vec()
}

fn mismatch(algorithm: &Algorithm, file: &str, expected: &str, actual: &str) -> Failure {
    Failure {
        // A wrong checksum is wrong on every attempt. Retrying would only read the same
        // bytes again and report the same thing.
        category: FailureKind::Permanent,
        message: format!("{file}: expected {expected}, got {actual}"),
        code: Some(algorithm.code("mismatch")),
        params: vec![("file".to_owned(), file.to_owned())],
    }
}

/// A sidecar none of whose files is here, or that lists none: nothing it promises was checked.
fn unverifiable(algorithm: &Algorithm, name: &str, problem: sidecar::Unverifiable) -> Failure {
    let (code, message, file) = match problem {
        sidecar::Unverifiable::Missing(file) => (
            "missing",
            format!("none of the files {name} lists is in this package, {file} among them"),
            file,
        ),
        sidecar::Unverifiable::Empty => (
            "empty",
            format!("{name} lists no {} checksum", algorithm.label),
            name.to_owned(),
        ),
    };
    Failure {
        // The package's files do not change between attempts, so neither would the answer.
        category: FailureKind::Permanent,
        message,
        code: Some(algorithm.code(code)),
        params: vec![("file".to_owned(), file)],
    }
}

/// The warning for listed files the package lacks while others verified: a release split
/// across packages, say. Named, so a pass is never read as "all".
fn unchecked_warning(algorithm: &Algorithm, files: &[String]) -> LabelPart {
    /// Names spelt out; a longer list is counted, since a step line is no inventory.
    const NAMED: usize = 20;
    let named = files
        .iter()
        .take(NAMED)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    // The count says how many there are; the English text also says how many went unnamed.
    let more = files
        .len()
        .checked_sub(NAMED)
        .filter(|more| *more > 0)
        .map_or_else(String::new, |more| format!(", and {more} more"));
    LabelPart {
        code: algorithm.code("unchecked"),
        message: format!(
            "{} checksums matched, but {} listed file(s) are not in this package and were not checked: {named}{more}",
            algorithm.label,
            files.len()
        ),
        params: vec![
            ("count".to_owned(), files.len().to_string()),
            ("files".to_owned(), named),
        ],
    }
}

fn refuse(algorithm: &Algorithm, message: String) -> Failure {
    Failure {
        category: FailureKind::Permanent,
        message,
        code: Some(algorithm.code("mismatch")),
        params: Vec::new(),
    }
}
