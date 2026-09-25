use std::path::Path;

use anyhow::Result;
use crc32fast::Hasher as Crc32Hasher;
use md5::Md5;
use rd_core::ChecksumAlgorithm;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

/// A computed checksum encoded in lowercase hexadecimal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComputedChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: String,
}

/// Streams a file once and computes the requested digest.
pub async fn compute_checksum(
    path: &Path,
    algorithm: ChecksumAlgorithm,
) -> Result<ComputedChecksum> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let value = match algorithm {
        ChecksumAlgorithm::Md5 => {
            let mut hasher = Md5::new();
            update_digest(&mut file, &mut buffer, &mut hasher).await?;
            hex::encode(hasher.finalize())
        }
        ChecksumAlgorithm::Sha1 => {
            let mut hasher = Sha1::new();
            update_digest(&mut file, &mut buffer, &mut hasher).await?;
            hex::encode(hasher.finalize())
        }
        ChecksumAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            update_digest(&mut file, &mut buffer, &mut hasher).await?;
            hex::encode(hasher.finalize())
        }
        ChecksumAlgorithm::Crc32 => {
            let mut hasher = Crc32Hasher::new();
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            format!("{:08x}", hasher.finalize())
        }
        ChecksumAlgorithm::DropboxContentHash => {
            let mut hasher = DropboxContentHasher::default();
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            hex::encode(hasher.finalize())
        }
    };
    Ok(ComputedChecksum { algorithm, value })
}

/// The size of one block of Dropbox's `content_hash`.
const DROPBOX_BLOCK_BYTES: usize = 4 * 1024 * 1024;

/// Dropbox's `content_hash`, as its documentation defines it: the file is split into 4 MiB
/// blocks, each block is hashed with SHA-256, and the concatenation of those digests is hashed
/// with SHA-256 again. A file shorter than one block therefore hashes to
/// `sha256(sha256(bytes))`, and an empty file to `sha256("")`.
///
/// Streamed like the others: the block hasher is fed as bytes arrive and closed at every block
/// boundary, so a file is read once whatever its size.
#[derive(Default)]
struct DropboxContentHasher {
    outer: Sha256,
    block: Sha256,
    filled: usize,
}

impl DropboxContentHasher {
    fn update(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let room = DROPBOX_BLOCK_BYTES - self.filled;
            let take = room.min(bytes.len());
            self.block.update(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == DROPBOX_BLOCK_BYTES {
                self.close_block();
            }
        }
    }

    fn close_block(&mut self) {
        let block = std::mem::take(&mut self.block);
        self.outer.update(block.finalize());
        self.filled = 0;
    }

    fn finalize(mut self) -> [u8; 32] {
        if self.filled > 0 {
            self.close_block();
        }
        self.outer.finalize().into()
    }
}

async fn update_digest<D: Digest>(
    file: &mut tokio::fs::File,
    buffer: &mut [u8],
    digest: &mut D,
) -> Result<()> {
    loop {
        let read = file.read(buffer).await?;
        if read == 0 {
            return Ok(());
        }
        digest.update(&buffer[..read]);
    }
}

/// Whether the file at `path` starts with the PAR2 packet magic (`PAR2\0PKT`).
///
/// The content check SABnzbd's `is_par2_file` performs in `handle_par2`: an obfuscated post
/// names its recovery data anything at all, and the header is the one thing the poster cannot
/// hide. It lives here rather than in the post-processor because the Usenet transport asks the
/// question too, the moment an assembled file is on disk (RD-108-23), and a transport must not
/// link the repair and archive stack for an eight-byte read.
#[must_use]
pub fn has_par2_magic(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0_u8; 8];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok_and(|()| &magic == b"PAR2\0PKT")
}

#[cfg(test)]
mod tests {
    use rd_core::ChecksumAlgorithm;
    use sha2::{Digest, Sha256};

    use super::{DROPBOX_BLOCK_BYTES, compute_checksum};

    async fn dropbox_hash_of(bytes: &[u8]) -> String {
        let file = tempfile::NamedTempFile::new().expect("a temporary file");
        std::fs::write(file.path(), bytes).expect("write");
        compute_checksum(file.path(), ChecksumAlgorithm::DropboxContentHash)
            .await
            .expect("a checksum")
            .value
    }

    /// A file shorter than one block is `sha256(sha256(bytes))`, not `sha256(bytes)` — the
    /// mistake that would make every Dropbox download fail its verification.
    #[tokio::test]
    async fn a_short_file_hashes_its_single_block_digest() {
        let bytes = b"hello dropbox";
        let inner = Sha256::digest(bytes);
        let expected = hex::encode(Sha256::digest(inner));
        assert_eq!(dropbox_hash_of(bytes).await, expected);
        assert_ne!(expected, hex::encode(inner));
        // The empty file is the digest of nothing at all.
        assert_eq!(dropbox_hash_of(b"").await, hex::encode(Sha256::digest(b"")));
    }

    /// A file one byte longer than a block hashes two block digests, the second of one byte.
    #[tokio::test]
    async fn a_file_longer_than_a_block_hashes_each_block_separately() {
        let mut bytes = vec![0xAB_u8; DROPBOX_BLOCK_BYTES];
        bytes.push(0xCD);
        let mut outer = Sha256::new();
        outer.update(Sha256::digest(&bytes[..DROPBOX_BLOCK_BYTES]));
        outer.update(Sha256::digest([0xCD_u8]));
        assert_eq!(dropbox_hash_of(&bytes).await, hex::encode(outer.finalize()));
    }
}
