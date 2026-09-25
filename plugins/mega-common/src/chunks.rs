//! Where MEGA's chunks end.
//!
//! The provider's own layout, and the host needs it twice over: a chunk MAC is sequential
//! inside a chunk and independent between chunks, so these offsets are both where the
//! integrity value is accumulated and the only offsets a parallel connection may start at.
//!
//! The sizes are 128 KiB times one through eight, and a mebibyte from there on. Measured
//! against the public 10 000 000-byte example on 2026-09-21: fourteen chunks, the last of
//! them 38 528 bytes.

/// Bytes the first chunk holds, and the step the first eight grow by.
pub const STEP: u64 = 128 * 1024;
/// Every chunk from the ninth on.
pub const PLATEAU: u64 = 1024 * 1024;
/// How many chunks grow before the plateau.
pub const RAMP: u64 = 8;

/// Absolute plaintext offsets where one chunk ends, the last one the file's size.
///
/// A zero-length file has no chunk and therefore no integrity value to accumulate; the caller
/// answers with a description that carries no integrity at all rather than an empty list,
/// which `rd-core` refuses.
#[must_use]
pub fn boundaries(size: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut position = 0_u64;
    let mut index = 1_u64;
    while position < size {
        let length = if index <= RAMP { STEP * index } else { PLATEAU };
        position = position.saturating_add(length).min(size);
        offsets.push(position);
        index += 1;
    }
    offsets
}

#[cfg(test)]
#[path = "chunks_tests.rs"]
mod chunks_tests;
