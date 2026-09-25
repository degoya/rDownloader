//! The component: verify every `.sha256` sidecar the package carries.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "postprocess-plugin",
});

use exports::rdownloader::plugin::postprocess::{Guest, StepEnd, StepInput};
use rdownloader::plugin::{
    source,
    types::{Failure, FailureKind},
};
use sha2::{Digest, Sha256};

use crate::sidecar;

struct Component;

/// Bytes read per call. Large enough to be worth the crossing, small enough that one read
/// cannot claim the whole response budget.
const CHUNK: u32 = 256 * 1024;

/// Longest sidecar read. A checksum file is a list of lines; anything larger is not one.
const MAX_SIDECAR: u64 = 4 * 1024 * 1024;

impl Guest for Component {
    fn run(input: StepInput) -> StepEnd {
        let sidecars: Vec<&String> = input
            .files
            .iter()
            .filter(|name| sidecar::is_sidecar(name))
            .collect();
        // No sidecar is not a failure: most packages have none, and a step that reported one
        // would fail every package in a category it was switched on for.
        if sidecars.is_empty() {
            return StepEnd::Skipped;
        }
        // Everything the sidecars ask for, flattened and ordered, so a checkpoint is just
        // "how many of these are done" and resuming needs no bookkeeping of its own.
        let mut wanted = Vec::new();
        for name in sidecars {
            let text = match read_text(&input.handle, name) {
                Ok(text) => text,
                Err(failure) => return StepEnd::Failed(failure),
            };
            for entry in sidecar::parse(&text) {
                // A sidecar may list files that are not in this package — a release split
                // across two folders does that. Verifying what is here is the useful answer.
                if input.files.contains(&entry.file) {
                    wanted.push(entry);
                }
            }
        }
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
            match digest_of(&input.handle, &entry.file) {
                Ok(digest) if digest == entry.digest => {}
                Ok(digest) => {
                    return StepEnd::Failed(mismatch(&entry.file, &entry.digest, &digest));
                }
                Err(failure) => return StepEnd::Failed(failure),
            }
            source::progress((index + 1) as u64, Some(total as u64));
        }
        StepEnd::Complete(None)
    }
}

/// Reads one file whole, for a sidecar small enough to hold in memory.
fn read_text(handle: &str, name: &str) -> Result<String, Failure> {
    let size = source::size_of(handle, name)?;
    if size > MAX_SIDECAR {
        return Err(refuse(format!("{name} is too large to be a checksum file")));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let chunk = source::read_at(handle, name, bytes.len() as u64, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| refuse(format!("{name} is not text")))
}

/// Streams one file through the hash, so a package larger than memory still verifies.
fn digest_of(handle: &str, name: &str) -> Result<String, Failure> {
    let size = source::size_of(handle, name)?;
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    while offset < size {
        let chunk = source::read_at(handle, name, offset, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        offset += chunk.len() as u64;
        hasher.update(&chunk);
    }
    Ok(sidecar::to_hex(&hasher.finalize()))
}

fn u32_bytes(value: usize) -> Vec<u8> {
    u32::try_from(value)
        .unwrap_or(u32::MAX)
        .to_le_bytes()
        .to_vec()
}

fn mismatch(file: &str, expected: &str, actual: &str) -> Failure {
    Failure {
        // A wrong checksum is wrong on every attempt. Retrying would only read the same
        // bytes again and report the same thing.
        category: FailureKind::Permanent,
        message: format!("{file}: expected {expected}, got {actual}"),
        code: Some("sha256_postprocess.mismatch".to_owned()),
        params: vec![("file".to_owned(), file.to_owned())],
    }
}

fn refuse(message: String) -> Failure {
    Failure {
        category: FailureKind::Permanent,
        message,
        code: Some("sha256_postprocess.mismatch".to_owned()),
        params: Vec::new(),
    }
}

export!(Component);
