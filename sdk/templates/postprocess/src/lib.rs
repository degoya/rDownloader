//! A scaffold post-processing step. It compiles, packages and passes conformance as it is.
//!
//! It computes the CRC-32 of every file in the package and logs it — small enough to read in
//! one sitting, and it shows the part every real step needs: reading a package in chunks, and
//! stopping half-way through a file without losing what was already done.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - **You never name a file.** `handle` plus the names in `files` is all there is, and both
//!   only mean anything inside this invocation — a path would be a way out of the package.
//! - **Stopping is normal.** Check `should-stop` and return `stopped` with a checkpoint; the
//!   host stores it and hands it back, so a service restart resumes instead of starting over.
//!   The checkpoint has to hold *everything* the resumed run needs — here the file, the offset
//!   and the checksum so far. An offset alone would resume with a checksum of nothing.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// How far the step has got: the checkpoint, in memory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// Index into the step's `files`.
    pub file: u32,
    /// Bytes of that file already summed.
    pub offset: u64,
    /// The running CRC-32 register over those bytes.
    pub crc: Crc32,
}

impl Progress {
    /// The checkpoint the host stores: 16 bytes, little-endian.
    #[must_use]
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&self.file.to_le_bytes());
        bytes.extend_from_slice(&self.offset.to_le_bytes());
        bytes.extend_from_slice(&self.crc.0.to_le_bytes());
        bytes
    }

    /// A checkpoint read back, or the start when there is none or it is not one of ours — a
    /// step that cannot read its checkpoint starts over rather than guessing.
    #[must_use]
    pub fn from_bytes(bytes: Option<&[u8]>) -> Self {
        bytes.and_then(Self::decode).unwrap_or_default()
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 16 {
            return None;
        }
        Some(Self {
            file: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            offset: u64::from_le_bytes(bytes[4..12].try_into().ok()?),
            crc: Crc32(u32::from_le_bytes(bytes[12..16].try_into().ok()?)),
        })
    }
}

/// CRC-32 (IEEE 802.3, as zip and SFV files use it), fed in chunks.
///
/// Bitwise rather than table-driven: eight shifts a byte is slow next to a table, and a table is
/// a kilobyte of code in a component that is read, signed and shown to the person installing it.
/// Swap in a table if a step spends its time here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

impl Crc32 {
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                let carry = self.0 & 1;
                self.0 >>= 1;
                if carry == 1 {
                    self.0 ^= 0xEDB8_8320;
                }
            }
        }
    }

    /// The checksum of everything fed so far.
    #[must_use]
    pub fn value(self) -> u32 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Crc32, Progress};

    #[test]
    fn the_checksum_matches_the_published_check_value() {
        let mut crc = Crc32::default();
        crc.update(b"123456789");
        assert_eq!(crc.value(), 0xCBF4_3926);
        assert_eq!(Crc32::default().value(), 0);
    }

    #[test]
    fn feeding_in_chunks_is_feeding_at_once() {
        let mut whole = Crc32::default();
        whole.update(b"one package, many chunks");
        let mut parts = Crc32::default();
        for chunk in b"one package, many chunks".chunks(5) {
            parts.update(chunk);
        }
        assert_eq!(whole, parts);
    }

    #[test]
    fn a_checkpoint_resumes_exactly_where_it_stopped() {
        let mut crc = Crc32::default();
        crc.update(b"half");
        let stopped = Progress {
            file: 2,
            offset: 4,
            crc,
        };
        assert_eq!(
            Progress::from_bytes(Some(stopped.to_bytes().as_slice())),
            stopped
        );
    }

    #[test]
    fn a_checkpoint_that_is_not_ours_starts_over() {
        assert_eq!(Progress::from_bytes(None), Progress::default());
        assert_eq!(
            Progress::from_bytes(Some(&[1_u8, 2, 3][..])),
            Progress::default()
        );
    }
}
