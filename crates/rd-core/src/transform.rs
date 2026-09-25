//! What a provider's stream is, before it is a file (RD-110-33, ADR 0011).
//!
//! A few providers encrypt every file on the client and never hold the key themselves. The
//! bytes a plain `GET` returns are ciphertext, and turning them into the file somebody asked
//! for is the host's work, not the plugin's: the plugin knows the provider and the key
//! schedule, the host owns the byte stream and executes a fixed, reviewed set of primitives
//! on its own write path.
//!
//! This module is the description alone -- what crosses the plugin boundary, what is written
//! down, and what a resume compares against. The arithmetic lives in `rd-http`, next to the
//! write it transforms.
//!
//! Two properties are deliberately visible in the types:
//!
//! * **Key material is never part of the description.** [`CipherSpec`] carries a vault
//!   reference and nothing else, so a description can be serialised into a checkpoint, logged
//!   and shown without any care being taken. The bytes live in [`TransformKey`], which has no
//!   `Serialize`, no `Display` and a `Debug` that prints a placeholder.
//! * **An unknown primitive is a refusal, not a fallback.** The algorithm is a name, so the
//!   set can grow without a contract change; [`ContentTransform::validate`] is the one place
//!   that decides which names this build knows, and it answers with a stable code.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Failure, FailureKind};

/// AES-128 in counter mode, counter block `nonce` followed by a big-endian u64 block index.
pub const CIPHER_AES_128_CTR: &str = "aes-128-ctr";

/// A CBC-MAC per chunk, condensed by a second CBC-MAC and folded to eight bytes.
pub const INTEGRITY_CBC_MAC_CHAIN: &str = "cbc-mac-chain";

/// Stable code: the description names a cipher this build does not implement.
pub const CODE_CIPHER_UNKNOWN: &str = "transform.cipher_unknown";
/// Stable code: the description names an integrity algorithm this build does not implement.
pub const CODE_INTEGRITY_UNKNOWN: &str = "transform.integrity_unknown";
/// Stable code: a parameter is the wrong length, out of order or out of bounds.
pub const CODE_PARAMETERS_INVALID: &str = "transform.parameters_invalid";
/// Stable code: the description is complete but its key material could not be found.
pub const CODE_KEY_MISSING: &str = "transform.key_missing";
/// Stable code: the plaintext does not match the integrity value the provider published.
pub const CODE_INTEGRITY_MISMATCH: &str = "transform.integrity_mismatch";
/// Stable code: the checkpoint was written by a different description, so it is not resumed.
pub const CODE_CHECKPOINT_MISMATCH: &str = "transform.checkpoint_mismatch";

/// Block size of every primitive here, in bytes.
pub const BLOCK_BYTES: u64 = 16;

/// The largest chunk-boundary list a description may carry.
///
/// A boundary every mebibyte covers a 200 GiB file, which is past anything a hoster serves in
/// one piece, and the list arrives from a plugin -- so it is bounded rather than trusted.
pub const MAX_BOUNDARIES: usize = 200_000;

/// Key material a plugin handed over, on its way to the vault.
///
/// Not `Serialize`, not `Display`, and its `Debug` prints a placeholder: the acceptance
/// criterion "decryption keys never appear in logs or UI URLs" is a property of the type
/// rather than of everyone's care. The bytes are cleared when the last owner drops them, the
/// way `rd-secrets` already treats every other secret -- a key read out of the vault for one
/// transfer has no business outliving it in freed heap memory.
#[derive(Clone, PartialEq, Eq)]
pub struct TransformKey(Vec<u8>);

impl Drop for TransformKey {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.0.zeroize();
    }
}

impl TransformKey {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The raw bytes. Named so a reader of a call site sees that a secret is being exposed.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for TransformKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "TransformKey({} bytes, [redacted])",
            self.0.len()
        )
    }
}

/// The cipher applied to every byte the host writes.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CipherSpec {
    /// One of the `CIPHER_*` names. Anything else is refused.
    pub algorithm: String,
    /// Vault reference of the key, never the key. `None` between the plugin answering and
    /// the host putting the bytes away -- see [`ContentTransform::with_key_reference`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_reference: Option<String>,
    pub nonce: Vec<u8>,
    /// Block index the file's first plaintext byte sits at.
    pub first_block: u64,
}

/// How the plaintext is proved to be what the provider stored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IntegritySpec {
    /// One of the `INTEGRITY_*` names. Anything else is refused.
    pub algorithm: String,
    /// Absolute plaintext offsets where one chunk ends and the next begins, ascending, the
    /// last one the file's size.
    pub boundaries: Vec<u64>,
    pub iv: Vec<u8>,
    pub expected: Vec<u8>,
}

impl IntegritySpec {
    /// Start and end of the chunk with this index, or `None` past the last one.
    #[must_use]
    pub fn chunk_range(&self, index: usize) -> Option<(u64, u64)> {
        let end = *self.boundaries.get(index)?;
        let start = if index == 0 {
            0
        } else {
            self.boundaries[index - 1]
        };
        Some((start, end))
    }

    /// The index of the chunk `offset` falls in, or `None` past the end.
    #[must_use]
    pub fn chunk_of(&self, offset: u64) -> Option<usize> {
        self.boundaries.iter().position(|end| offset < *end)
    }

    /// The highest boundary at or below `offset`, which is where a resume may pick up.
    ///
    /// The MAC of a chunk is sequential, and only finished chunk MACs are written down, so a
    /// continuation that landed inside a chunk has to fetch that chunk again from its start.
    #[must_use]
    pub fn aligned_floor(&self, offset: u64) -> u64 {
        self.boundaries
            .iter()
            .copied()
            .rfind(|end| *end <= offset)
            .unwrap_or(0)
    }

    /// Whether `offset` is a chunk boundary (`0` and the file size included).
    #[must_use]
    pub fn is_boundary(&self, offset: u64) -> bool {
        offset == 0 || self.boundaries.contains(&offset)
    }

    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.boundaries.len()
    }
}

/// Everything the host needs to turn one provider's stream into the file behind it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContentTransform {
    pub cipher: CipherSpec,
    /// `None` when the provider publishes nothing to check the plaintext against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<IntegritySpec>,
}

impl ContentTransform {
    /// The same description with the vault reference the key was stored under.
    #[must_use]
    pub fn with_key_reference(mut self, reference: impl Into<String>) -> Self {
        self.cipher.key_reference = Some(reference.into());
        self
    }

    /// Whether this build knows every primitive named here, and whether the parameters fit.
    ///
    /// Called where a description is taken in -- from a plugin, and again from a checkpoint --
    /// so nothing further down has to ask whether a nonce is the right length.
    pub fn validate(&self) -> Result<(), Failure> {
        if self.cipher.algorithm != CIPHER_AES_128_CTR {
            return Err(refusal(
                CODE_CIPHER_UNKNOWN,
                format!(
                    "this build implements no cipher called `{}`",
                    sanitise(&self.cipher.algorithm)
                ),
            ));
        }
        if self.cipher.nonce.len() != 8 {
            return Err(refusal(
                CODE_PARAMETERS_INVALID,
                format!(
                    "{CIPHER_AES_128_CTR} needs an 8-byte nonce, got {}",
                    self.cipher.nonce.len()
                ),
            ));
        }
        let Some(integrity) = &self.integrity else {
            return Ok(());
        };
        if integrity.algorithm != INTEGRITY_CBC_MAC_CHAIN {
            return Err(refusal(
                CODE_INTEGRITY_UNKNOWN,
                format!(
                    "this build implements no integrity algorithm called `{}`",
                    sanitise(&integrity.algorithm)
                ),
            ));
        }
        if integrity.iv.len() != 16 {
            return Err(refusal(
                CODE_PARAMETERS_INVALID,
                format!(
                    "{INTEGRITY_CBC_MAC_CHAIN} needs a 16-byte IV, got {}",
                    integrity.iv.len()
                ),
            ));
        }
        if integrity.expected.len() != 8 {
            return Err(refusal(
                CODE_PARAMETERS_INVALID,
                format!(
                    "{INTEGRITY_CBC_MAC_CHAIN} condenses to 8 bytes, got {}",
                    integrity.expected.len()
                ),
            ));
        }
        if integrity.boundaries.is_empty() || integrity.boundaries.len() > MAX_BOUNDARIES {
            return Err(refusal(
                CODE_PARAMETERS_INVALID,
                format!(
                    "a chunk boundary list of {} is not between 1 and {MAX_BOUNDARIES}",
                    integrity.boundaries.len()
                ),
            ));
        }
        if integrity
            .boundaries
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
            || integrity.boundaries.first().copied() == Some(0)
        {
            return Err(refusal(
                CODE_PARAMETERS_INVALID,
                "chunk boundaries must be strictly ascending and above zero".to_owned(),
            ));
        }
        Ok(())
    }

    /// A short, stable identifier of this description, key reference included.
    ///
    /// What a checkpoint records so a continuation can tell "the same stream" from "somebody
    /// else's state". Every parameter that changes the plaintext is in it, so two descriptions
    /// that would write different bytes cannot share one.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"rdownloader.content-transform.v1");
        hash_part(&mut digest, self.cipher.algorithm.as_bytes());
        hash_part(
            &mut digest,
            self.cipher
                .key_reference
                .as_deref()
                .unwrap_or("")
                .as_bytes(),
        );
        hash_part(&mut digest, &self.cipher.nonce);
        hash_part(&mut digest, &self.cipher.first_block.to_be_bytes());
        match &self.integrity {
            None => hash_part(&mut digest, b""),
            Some(integrity) => {
                hash_part(&mut digest, integrity.algorithm.as_bytes());
                hash_part(&mut digest, &integrity.iv);
                hash_part(&mut digest, &integrity.expected);
                for boundary in &integrity.boundaries {
                    hash_part(&mut digest, &boundary.to_be_bytes());
                }
            }
        }
        hex::encode(digest.finalize())[..32].to_owned()
    }
}

/// Length-prefixed, so `("ab", "c")` and `("a", "bc")` cannot hash the same.
fn hash_part(digest: &mut Sha256, part: &[u8]) {
    digest.update((part.len() as u64).to_be_bytes());
    digest.update(part);
}

/// A refusal that ends the attempt rather than retrying it: the description will not improve.
fn refusal(code: &str, message: String) -> Failure {
    Failure::coded(FailureKind::Permanent, code, message)
}

/// Keeps a plugin-supplied name printable, so a refusal cannot smuggle control characters or
/// a wall of text into a log line.
fn sanitise(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_graphic() || *character == ' ')
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> CipherSpec {
        CipherSpec {
            algorithm: CIPHER_AES_128_CTR.to_owned(),
            key_reference: Some("vault://019d0000-0000-7000-8000-000000000001".to_owned()),
            nonce: vec![0x80, 0x1b, 0x72, 0xfd, 0x96, 0x41, 0xcc, 0xfa],
            first_block: 0,
        }
    }

    fn integrity() -> IntegritySpec {
        IntegritySpec {
            algorithm: INTEGRITY_CBC_MAC_CHAIN.to_owned(),
            boundaries: vec![131_072, 393_216, 524_288],
            iv: vec![0x11; 16],
            expected: vec![0x22; 8],
        }
    }

    fn transform() -> ContentTransform {
        ContentTransform {
            cipher: cipher(),
            integrity: Some(integrity()),
        }
    }

    #[test]
    fn a_complete_description_validates() {
        transform().validate().expect("the description is known");
    }

    #[test]
    fn an_unknown_cipher_is_refused_under_its_own_code() {
        let mut description = transform();
        description.cipher.algorithm = "rot13-ctr".to_owned();
        let failure = description.validate().expect_err("refused");
        assert_eq!(failure.code.as_deref(), Some(CODE_CIPHER_UNKNOWN));
        assert!(matches!(failure.category, FailureKind::Permanent));
        assert!(failure.message.contains("rot13-ctr"), "{}", failure.message);
    }

    #[test]
    fn an_unknown_integrity_algorithm_is_refused_under_its_own_code() {
        let mut description = transform();
        if let Some(integrity) = description.integrity.as_mut() {
            integrity.algorithm = "sha3-chain".to_owned();
        }
        let failure = description.validate().expect_err("refused");
        assert_eq!(failure.code.as_deref(), Some(CODE_INTEGRITY_UNKNOWN));
    }

    #[test]
    fn a_refusal_cannot_smuggle_control_characters_into_a_log_line() {
        let mut description = transform();
        description.cipher.algorithm = "aes\n\u{7}INJECTED".to_owned();
        let failure = description.validate().expect_err("refused");
        assert!(!failure.message.contains('\n'), "{}", failure.message);
        assert!(!failure.message.contains('\u{7}'), "{}", failure.message);
    }

    #[test]
    fn bad_parameter_lengths_are_refused() {
        for mutate in [
            (|d: &mut ContentTransform| d.cipher.nonce = vec![0; 7]) as fn(&mut ContentTransform),
            |d: &mut ContentTransform| {
                d.integrity.as_mut().expect("integrity").iv = vec![0; 15];
            },
            |d: &mut ContentTransform| {
                d.integrity.as_mut().expect("integrity").expected = vec![0; 16];
            },
            |d: &mut ContentTransform| {
                d.integrity.as_mut().expect("integrity").boundaries = Vec::new();
            },
            |d: &mut ContentTransform| {
                d.integrity.as_mut().expect("integrity").boundaries = vec![10, 10];
            },
            |d: &mut ContentTransform| {
                d.integrity.as_mut().expect("integrity").boundaries = vec![0, 10];
            },
        ] {
            let mut description = transform();
            mutate(&mut description);
            let failure = description.validate().expect_err("refused");
            assert_eq!(failure.code.as_deref(), Some(CODE_PARAMETERS_INVALID));
        }
    }

    #[test]
    fn a_description_without_integrity_still_validates() {
        let description = ContentTransform {
            cipher: cipher(),
            integrity: None,
        };
        description.validate().expect("a cipher alone is complete");
    }

    #[test]
    fn chunk_ranges_cover_the_file_without_a_gap() {
        let integrity = integrity();
        assert_eq!(integrity.chunk_range(0), Some((0, 131_072)));
        assert_eq!(integrity.chunk_range(1), Some((131_072, 393_216)));
        assert_eq!(integrity.chunk_range(2), Some((393_216, 524_288)));
        assert_eq!(integrity.chunk_range(3), None);
        assert_eq!(integrity.chunk_of(0), Some(0));
        assert_eq!(integrity.chunk_of(131_071), Some(0));
        assert_eq!(integrity.chunk_of(131_072), Some(1));
        assert_eq!(integrity.chunk_of(524_288), None);
    }

    #[test]
    fn a_resume_falls_back_to_the_last_finished_chunk() {
        let integrity = integrity();
        assert_eq!(integrity.aligned_floor(0), 0);
        assert_eq!(integrity.aligned_floor(131_071), 0);
        assert_eq!(integrity.aligned_floor(131_072), 131_072);
        assert_eq!(integrity.aligned_floor(200_000), 131_072);
        assert_eq!(integrity.aligned_floor(524_288), 524_288);
        assert!(integrity.is_boundary(0));
        assert!(integrity.is_boundary(393_216));
        assert!(!integrity.is_boundary(200_000));
    }

    /// Every parameter that changes the plaintext changes the fingerprint. A continuation is
    /// only allowed to reuse somebody's chunk MACs when nothing that produced them moved.
    #[test]
    fn the_fingerprint_moves_with_every_parameter_that_changes_the_bytes() {
        let base = transform().fingerprint();
        assert_eq!(base.len(), 32);
        assert_eq!(base, transform().fingerprint(), "not deterministic");
        let mutations: [fn(&mut ContentTransform); 6] = [
            |d| d.cipher.nonce[0] ^= 1,
            |d| d.cipher.first_block += 1,
            |d| d.cipher.key_reference = Some("vault://other".to_owned()),
            |d| d.integrity.as_mut().expect("integrity").iv[3] ^= 1,
            |d| d.integrity.as_mut().expect("integrity").expected[0] ^= 1,
            |d| d.integrity.as_mut().expect("integrity").boundaries[0] += 16,
        ];
        for mutate in mutations {
            let mut description = transform();
            mutate(&mut description);
            assert_ne!(base, description.fingerprint());
        }
    }

    /// The length prefix is what stops two different descriptions hashing the same.
    #[test]
    fn concatenation_cannot_collide() {
        let mut left = transform();
        left.cipher.algorithm = CIPHER_AES_128_CTR.to_owned();
        left.cipher.key_reference = Some("ab".to_owned());
        let mut right = left.clone();
        right.cipher.key_reference = Some("a".to_owned());
        assert_ne!(left.fingerprint(), right.fingerprint());
    }

    /// Nothing that can be serialised, printed or persisted carries key bytes.
    #[test]
    fn a_description_carries_a_reference_and_never_a_key() {
        let json = serde_json::to_string(&transform()).expect("json");
        assert!(json.contains("vault://"), "{json}");
        assert!(!json.contains("key\":["), "{json}");
        let key = TransformKey::new(vec![0xAB; 16]);
        let printed = format!("{key:?}");
        assert!(printed.contains("[redacted]"), "{printed}");
        assert!(!printed.contains("ab"), "{printed}");
        assert!(!printed.contains("171"), "{printed}");
        assert_eq!(key.len(), 16);
        assert!(!key.is_empty());
        assert_eq!(key.expose(), [0xAB; 16]);
    }
}
