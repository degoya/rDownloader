//! The encryption stream an archive is written through (RD-160-01).
//!
//! A database copy can be gigabytes, so the archive is sealed in chunks rather than in one
//! call: the STREAM construction (Hoang, Reyhanitabar, Rogaway, Vizar) over XChaCha20-Poly1305,
//! the cipher the settings bundle already uses. Every chunk of up to [`CHUNK_LEN`] bytes is
//! sealed on its own under the nonce `prefix || counter || last`: 19 random bytes per archive, a
//! big-endian chunk counter, and a flag that is 1 only on the final chunk. The header is the
//! associated data of every chunk. What that buys:
//!
//! * a changed byte anywhere fails its chunk's tag;
//! * chunks cannot be reordered or dropped, because the counter is in the nonce;
//! * a truncated archive fails, because the chunk it now ends with was not sealed as the last;
//! * the header — the salt a restore derives the key from — cannot be swapped undetected.
//!
//! A wrong passphrase and a damaged archive look the same here: both fail the first tag.
//!
//! Layout: `RDBACKUP`, container version, KDF id, the three Argon2id parameters (u32 LE), the
//! salt, the nonce prefix, the chunk length (u32 LE); then the sealed chunks, each its
//! ciphertext followed by a 16-byte tag. The final chunk may be empty.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::Rng;

use crate::crypto::{BackupKey, KDF_M_COST, KDF_P_COST, KDF_T_COST, SALT_LEN};

/// The first eight bytes of every archive.
pub const MAGIC: &[u8; 8] = b"RDBACKUP";
/// The container version this build writes and reads.
pub const CONTAINER_VERSION: u8 = 1;
/// Plaintext bytes per sealed chunk.
pub const CHUNK_LEN: usize = 1 << 20;
/// Length of the header in bytes.
pub const HEADER_LEN: usize = 8 + 1 + 1 + 12 + SALT_LEN + NONCE_PREFIX_LEN + 4;

const KDF_ARGON2ID: u8 = 1;
const TAG_LEN: usize = 16;
const NONCE_PREFIX_LEN: usize = 19;
/// Largest chunk a reader accepts, so a forged header cannot make it allocate without bound.
const MAX_CHUNK_LEN: usize = 16 << 20;

/// The plaintext header of an archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    /// The Argon2id salt the archive's key was derived with.
    pub salt: [u8; SALT_LEN],
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    chunk_len: u32,
}

impl Header {
    fn fresh(salt: [u8; SALT_LEN]) -> Self {
        let mut nonce_prefix = [0_u8; NONCE_PREFIX_LEN];
        rand::rng().fill_bytes(&mut nonce_prefix);
        Self {
            salt,
            nonce_prefix,
            chunk_len: u32::try_from(CHUNK_LEN).unwrap_or(u32::MAX),
        }
    }

    fn encode(&self) -> [u8; HEADER_LEN] {
        let mut bytes = [0_u8; HEADER_LEN];
        let mut at = 0;
        let mut put = |part: &[u8]| {
            bytes[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        };
        put(MAGIC);
        put(&[CONTAINER_VERSION, KDF_ARGON2ID]);
        put(&KDF_M_COST.to_le_bytes());
        put(&KDF_T_COST.to_le_bytes());
        put(&KDF_P_COST.to_le_bytes());
        put(&self.salt);
        put(&self.nonce_prefix);
        put(&self.chunk_len.to_le_bytes());
        bytes
    }

    /// Reads and checks a header; refuses anything this build did not write.
    ///
    /// # Errors
    ///
    /// When the bytes are not an archive header of this version and these KDF parameters.
    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0_u8; HEADER_LEN];
        reader.read_exact(&mut bytes)?;
        let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
        if &bytes[..8] != MAGIC {
            return Err(invalid("not an rDownloader backup"));
        }
        if bytes[8] != CONTAINER_VERSION || bytes[9] != KDF_ARGON2ID {
            return Err(invalid("unsupported backup container version"));
        }
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if (word(10), word(14), word(18)) != (KDF_M_COST, KDF_T_COST, KDF_P_COST) {
            return Err(invalid("unsupported backup key derivation parameters"));
        }
        let mut salt = [0_u8; SALT_LEN];
        salt.copy_from_slice(&bytes[22..22 + SALT_LEN]);
        let mut nonce_prefix = [0_u8; NONCE_PREFIX_LEN];
        let start = 22 + SALT_LEN;
        nonce_prefix.copy_from_slice(&bytes[start..start + NONCE_PREFIX_LEN]);
        let chunk_len = word(start + NONCE_PREFIX_LEN);
        if chunk_len == 0 || chunk_len as usize > MAX_CHUNK_LEN {
            return Err(invalid("unsupported backup chunk length"));
        }
        Ok(Self {
            salt,
            nonce_prefix,
            chunk_len,
        })
    }
}

/// Reads the header of an archive on disk: the salt a restore derives the key from.
///
/// # Errors
///
/// When the file cannot be read or is not an archive.
pub fn read_header(path: &Path) -> io::Result<Header> {
    Header::read(&mut std::fs::File::open(path)?)
}

fn nonce(prefix: &[u8; NONCE_PREFIX_LEN], counter: u32, last: bool) -> XNonce {
    let mut bytes = [0_u8; 24];
    bytes[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    bytes[NONCE_PREFIX_LEN..NONCE_PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    bytes[23] = u8::from(last);
    XNonce::from(bytes)
}

fn cipher(key: &BackupKey) -> io::Result<XChaCha20Poly1305> {
    XChaCha20Poly1305::new_from_slice(key.key_bytes())
        .map_err(|_| io::Error::other("invalid backup key length"))
}

/// Seals everything written to it; [`SealingWriter::finish`] writes the final chunk.
///
/// Dropped without `finish`, the stream has no final chunk and no reader accepts it — an
/// archive that was not finished can never pass for one that was.
pub struct SealingWriter<W: Write> {
    inner: W,
    cipher: XChaCha20Poly1305,
    header: Header,
    associated: [u8; HEADER_LEN],
    counter: u32,
    buffer: Vec<u8>,
}

impl<W: Write> SealingWriter<W> {
    /// Writes the header and starts the stream.
    ///
    /// # Errors
    ///
    /// When the header cannot be written.
    pub fn new(mut inner: W, key: &BackupKey) -> io::Result<Self> {
        let header = Header::fresh(key.salt());
        let associated = header.encode();
        inner.write_all(&associated)?;
        Ok(Self {
            inner,
            cipher: cipher(key)?,
            header,
            associated,
            counter: 0,
            buffer: Vec::with_capacity(CHUNK_LEN),
        })
    }

    fn seal(&mut self, length: usize, last: bool) -> io::Result<()> {
        let sealed = self
            .cipher
            .encrypt(
                &nonce(&self.header.nonce_prefix, self.counter, last),
                Payload {
                    msg: &self.buffer[..length],
                    aad: &self.associated,
                },
            )
            .map_err(|_| io::Error::other("seal backup chunk"))?;
        self.inner.write_all(&sealed)?;
        self.buffer.drain(..length);
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("backup archive exceeds the chunk counter"))?;
        Ok(())
    }

    /// Seals what is left as the final chunk and hands back the inner writer.
    ///
    /// # Errors
    ///
    /// When the final chunk cannot be sealed or written.
    pub fn finish(mut self) -> io::Result<W> {
        let remaining = self.buffer.len();
        self.seal(remaining, true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for SealingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(data);
        // Strictly more than a chunk: a full chunk is sealed as not-last only once it is known
        // that something follows it, so the final chunk is always sealed by `finish`.
        while self.buffer.len() > CHUNK_LEN {
            self.seal(CHUNK_LEN, false)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Opens a sealed stream; every byte it hands out passed its chunk's tag.
pub struct OpeningReader<R: Read> {
    inner: BufReader<R>,
    cipher: XChaCha20Poly1305,
    header: Header,
    associated: [u8; HEADER_LEN],
    counter: u32,
    plain: Vec<u8>,
    position: usize,
    finished: bool,
}

impl<R: Read> OpeningReader<R> {
    /// Reads the header and checks that it was written for this key's salt.
    ///
    /// # Errors
    ///
    /// When the header is not an archive header, or names another salt than the key's.
    pub fn new(inner: R, key: &BackupKey) -> io::Result<Self> {
        let mut inner = BufReader::new(inner);
        let header = Header::read(&mut inner)?;
        if header.salt != key.salt() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the backup was sealed under another passphrase",
            ));
        }
        let associated = header.encode();
        Ok(Self {
            inner,
            cipher: cipher(key)?,
            header,
            associated,
            counter: 0,
            plain: Vec::new(),
            position: 0,
            finished: false,
        })
    }

    fn open_next(&mut self) -> io::Result<()> {
        let sealed_len = self.header.chunk_len as usize + TAG_LEN;
        let mut sealed = Vec::with_capacity(sealed_len);
        (&mut self.inner)
            .take(sealed_len as u64)
            .read_to_end(&mut sealed)?;
        let last = self.inner.fill_buf()?.is_empty();
        let damaged = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the backup is damaged or the passphrase is wrong",
            )
        };
        if sealed.len() < TAG_LEN {
            return Err(damaged());
        }
        self.plain = self
            .cipher
            .decrypt(
                &nonce(&self.header.nonce_prefix, self.counter, last),
                Payload {
                    msg: &sealed,
                    aad: &self.associated,
                },
            )
            .map_err(|_| damaged())?;
        self.position = 0;
        self.finished = last;
        self.counter = self.counter.checked_add(1).ok_or_else(damaged)?;
        Ok(())
    }
}

impl<R: Read> Read for OpeningReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.position == self.plain.len() {
            if self.finished {
                return Ok(0);
            }
            self.open_next()?;
        }
        let count = out.len().min(self.plain.len() - self.position);
        out[..count].copy_from_slice(&self.plain[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::{CHUNK_LEN, HEADER_LEN, OpeningReader, SealingWriter};
    use crate::crypto::BackupKey;

    async fn key() -> BackupKey {
        BackupKey::derive_new("correct horse").await.expect("key")
    }

    fn seal(key: &BackupKey, plain: &[u8]) -> Vec<u8> {
        let mut writer = SealingWriter::new(Vec::new(), key).expect("writer");
        // In uneven pieces, so chunk boundaries fall inside writes.
        for piece in plain.chunks(7_919) {
            writer.write_all(piece).expect("write");
        }
        writer.finish().expect("finish")
    }

    fn open(key: &BackupKey, sealed: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut plain = Vec::new();
        OpeningReader::new(sealed, key)?.read_to_end(&mut plain)?;
        Ok(plain)
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    #[tokio::test]
    async fn every_length_round_trips_including_the_chunk_boundaries() {
        let key = key().await;
        for len in [
            0,
            1,
            CHUNK_LEN - 1,
            CHUNK_LEN,
            CHUNK_LEN + 1,
            2 * CHUNK_LEN,
            2 * CHUNK_LEN + 5,
        ] {
            let plain = sample(len);
            assert_eq!(
                open(&key, &seal(&key, &plain)).expect("open"),
                plain,
                "{len}"
            );
        }
    }

    #[tokio::test]
    async fn the_plaintext_does_not_appear_in_the_sealed_stream() {
        let key = key().await;
        let canary = b"password-canary-that-must-never-be-readable";
        let sealed = seal(&key, canary);
        assert!(!sealed.windows(canary.len()).any(|window| window == canary));
    }

    #[tokio::test]
    async fn a_changed_byte_a_truncation_and_another_passphrase_are_all_refused() {
        let key = key().await;
        let plain = sample(2 * CHUNK_LEN + 17);
        let sealed = seal(&key, &plain);

        let mut flipped = sealed.clone();
        flipped[HEADER_LEN + 100] ^= 1;
        assert!(open(&key, &flipped).is_err());

        let mut header_changed = sealed.clone();
        header_changed[HEADER_LEN - 1] ^= 1;
        assert!(open(&key, &header_changed).is_err());

        // Cut exactly after the first chunk: what is left is a complete, valid-looking chunk,
        // but it was not sealed as the last one.
        let first_chunk_end = HEADER_LEN + CHUNK_LEN + 16;
        assert!(open(&key, &sealed[..first_chunk_end]).is_err());
        assert!(open(&key, &sealed[..sealed.len() - 1]).is_err());

        let other = BackupKey::derive("wrong horse", key.salt())
            .await
            .expect("key");
        assert!(open(&other, &sealed).is_err());
        let fresh = BackupKey::derive_new("correct horse").await.expect("key");
        assert!(open(&fresh, &sealed).is_err(), "another salt");
    }

    #[tokio::test]
    async fn a_writer_dropped_without_finishing_leaves_nothing_a_reader_accepts() {
        let key = key().await;
        let mut sink = Vec::new();
        {
            let mut writer = SealingWriter::new(&mut sink, &key).expect("writer");
            writer.write_all(&sample(3 * CHUNK_LEN)).expect("write");
        }
        assert!(open(&key, &sink).is_err());
    }
}
