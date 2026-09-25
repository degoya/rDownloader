//! Turning a provider's stream into the file behind it, on the write path (RD-110-33).
//!
//! ADR 0011 put the cipher here rather than in a plugin for one structural reason: the engine
//! already writes at an absolute offset, and AES-CTR is seekable at any 16-byte boundary. So a
//! decryption is a transform on a buffer that is already allocated at a position that is
//! already known -- no second pass over the file, no second copy on disk, and nothing about
//! chunking, ranges, checkpoints or the limiter changes.
//!
//! What is stateful is the integrity check, and only within one provider chunk. The MAC of a
//! chunk is sequential; between chunks it is independent. That is what keeps parallel
//! connections possible, and it is also the constraint this module enforces: a run whose
//! connection boundaries do not line up with the provider's chunk boundaries gives up its
//! parallelism rather than condensing a MAC it cannot compute.
//!
//! Nothing here reaches for a key by itself. [`StreamTransform`] is built from a description
//! and the bytes the caller resolved out of `rd-secrets`, and the bytes go no further: the
//! type has no `Debug`, no `Display` and no `Serialize`.

use std::collections::BTreeMap;

use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit, KeyIvInit, StreamCipher, StreamCipherSeek},
};
use rd_core::{
    BLOCK_BYTES, CODE_CHECKPOINT_MISMATCH, CODE_INTEGRITY_MISMATCH, CODE_PARAMETERS_INVALID,
    ContentTransform, Failure, FailureKind, IntegritySpec, TransformKey,
};

use subtle::ConstantTimeEq as _;
use zeroize::Zeroize as _;

use crate::ChunkSpec;

type Aes128Ctr = ctr::Ctr128BE<Aes128>;

/// Bytes of one AES block, as a `usize` for slicing.
const BLOCK: usize = BLOCK_BYTES as usize;

/// What a previous attempt left behind: which description wrote it, and what it finished.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransformCheckpoint {
    /// [`ContentTransform::fingerprint`] of the description that produced these MACs.
    pub fingerprint: Option<String>,
    /// Finished provider-chunk MACs, by chunk index.
    pub macs: Vec<(usize, [u8; BLOCK])>,
}

/// A validated description together with the key it needs.
pub struct StreamTransform {
    description: ContentTransform,
    fingerprint: String,
    key: [u8; BLOCK],
    nonce: [u8; 8],
    first_block: u64,
}

/// The key copy this transform works from is cleared when the transfer's transform goes away.
impl Drop for StreamTransform {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl StreamTransform {
    /// Refuses anything this build cannot compute, before a single byte is fetched.
    ///
    /// A description without a vault reference is one of those refusals, and deliberately so:
    /// the reference is what the fingerprint stands the key on, so a transform built before
    /// the key was put away could share a fingerprint with a different key's. It is also the
    /// point at which "key material is a secret from the moment it arrives" stops being a
    /// convention and becomes a precondition.
    pub fn new(description: ContentTransform, key: &TransformKey) -> Result<Self, Failure> {
        description.validate()?;
        if description.cipher.key_reference.is_none() {
            return Err(Failure::coded(
                FailureKind::Permanent,
                rd_core::CODE_KEY_MISSING,
                "this transform has no vault reference for its key".to_owned(),
            ));
        }
        let key: [u8; BLOCK] = key.expose().try_into().map_err(|_| {
            Failure::coded(
                FailureKind::Permanent,
                CODE_PARAMETERS_INVALID,
                format!("aes-128 needs a 16-byte key, got {}", key.len()),
            )
        })?;
        let nonce: [u8; 8] = description
            .cipher
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| {
                Failure::coded(
                    FailureKind::Permanent,
                    CODE_PARAMETERS_INVALID,
                    "the nonce is not 8 bytes".to_owned(),
                )
            })?;
        let first_block = description.cipher.first_block;
        Ok(Self {
            fingerprint: description.fingerprint(),
            description,
            key,
            nonce,
            first_block,
        })
    }

    #[must_use]
    pub fn description(&self) -> &ContentTransform {
        &self.description
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn integrity(&self) -> Option<&IntegritySpec> {
        self.description.integrity.as_ref()
    }

    /// The plaintext size the description implies, when it says anything about it.
    #[must_use]
    pub fn expected_size(&self) -> Option<u64> {
        self.integrity()
            .and_then(|integrity| integrity.boundaries.last().copied())
    }

    /// Turns the ciphertext in `buffer` into plaintext in place.
    ///
    /// `position` is the absolute offset of `buffer[0]` in the file. CTR is its own inverse,
    /// so the same call encrypts -- which is what the fixed-vector tests use.
    pub fn apply(&self, position: u64, buffer: &mut [u8]) {
        if buffer.is_empty() {
            return;
        }
        let block = self.first_block.wrapping_add(position / BLOCK_BYTES);
        let mut iv = [0_u8; BLOCK];
        iv[..8].copy_from_slice(&self.nonce);
        iv[8..].copy_from_slice(&block.to_be_bytes());
        let mut cipher = Aes128Ctr::new((&self.key).into(), (&iv).into());
        let within = position % BLOCK_BYTES;
        if within > 0 {
            cipher.seek(within);
        }
        cipher.apply_keystream(buffer);
    }

    /// A MAC accumulator for the provider chunk `position` falls in.
    #[must_use]
    pub fn mac_walker(&self, position: u64) -> Option<MacWalker<'_>> {
        let integrity = self.integrity()?;
        Some(MacWalker::new(
            integrity,
            Aes128::new((&self.key).into()),
            position,
        ))
    }

    /// Which recorded chunk MACs this attempt may carry over.
    ///
    /// A continuation whose description differs starts over rather than resuming somebody
    /// else's state -- the rule `transfer` already follows for its opaque checkpoints. The
    /// fingerprint covers every parameter that changes the plaintext, the key reference
    /// included, so a re-resolve that came back with a different key discards the MACs the
    /// old one wrote.
    #[must_use]
    pub fn adopt(&self, checkpoint: &TransformCheckpoint) -> BTreeMap<usize, [u8; BLOCK]> {
        if checkpoint.fingerprint.as_deref() != Some(self.fingerprint.as_str()) {
            return BTreeMap::new();
        }
        let Some(integrity) = self.integrity() else {
            return BTreeMap::new();
        };
        checkpoint
            .macs
            .iter()
            .filter(|(index, _)| *index < integrity.chunk_count())
            .map(|(index, mac)| (*index, *mac))
            .collect()
    }

    /// Condenses the finished chunk MACs and compares with what the provider published.
    ///
    /// A wrong key is otherwise indistinguishable from a correct download: the length
    /// matches, the name is right, and every byte is rubbish. Neither JDownloader nor pyLoad
    /// checks this.
    pub fn verify(&self, macs: &BTreeMap<usize, [u8; BLOCK]>) -> Result<(), Failure> {
        let Some(integrity) = self.integrity() else {
            return Ok(());
        };
        let mut chain = Vec::with_capacity(integrity.chunk_count() * BLOCK);
        for index in 0..integrity.chunk_count() {
            let Some(mac) = macs.get(&index) else {
                return Err(Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    CODE_CHECKPOINT_MISMATCH,
                    format!("chunk {index} of this stream was never accounted for"),
                ));
            };
            chain.extend_from_slice(mac);
        }
        let condensed = cbc_mac(&Aes128::new((&self.key).into()), &[0_u8; BLOCK], &chain);
        // The provider's fold: four big-endian words, the first pair xored into the first
        // half and the second pair into the second.
        let mut folded = [0_u8; 8];
        for (target, (left, right)) in folded[..4]
            .iter_mut()
            .zip(condensed[0..4].iter().zip(&condensed[4..8]))
        {
            *target = left ^ right;
        }
        for (target, (left, right)) in folded[4..]
            .iter_mut()
            .zip(condensed[8..12].iter().zip(&condensed[12..16]))
        {
            *target = left ^ right;
        }
        // Constant time, not `==`. The folded value is a function of the key, the expected
        // one is the provider's, and a comparison that stops at the first differing byte
        // tells an attacker who can re-run the download how far a guess got. `rd-authn`
        // compares its TOTP codes and recovery codes the same way; `subtle` is used here
        // rather than a third hand-written loop.
        if bool::from(folded.as_slice().ct_eq(integrity.expected.as_slice())) {
            return Ok(());
        }
        // Neither value is printed: one is the provider's and harmless, the other is a
        // function of the key, and a log line carrying both is a line that says how close a
        // guess was.
        Err(Failure::coded(
            FailureKind::Permanent,
            CODE_INTEGRITY_MISMATCH,
            "the decrypted file does not match the integrity value the provider published"
                .to_owned(),
        ))
    }
}

/// The CBC-MAC of one provider chunk, accumulated as the bytes are written.
///
/// Fed in whatever pieces the network delivers, which is never the block size, so a partial
/// block is buffered until the next call completes it. The last block of a chunk is padded
/// with zeros, which is what the provider's own definition says.
pub struct MacWalker<'a> {
    integrity: &'a IntegritySpec,
    /// The key schedule, expanded once rather than once per block.
    cipher: Aes128,
    /// The chunk being accumulated: its index, its end, and the running state.
    index: usize,
    end: u64,
    state: [u8; BLOCK],
    pending: [u8; BLOCK],
    pending_len: usize,
    /// Where the next byte is expected, so a gap is refused rather than silently MACed.
    position: u64,
    exhausted: bool,
}

impl<'a> MacWalker<'a> {
    fn new(integrity: &'a IntegritySpec, cipher: Aes128, position: u64) -> Self {
        let index = integrity.chunk_of(position).unwrap_or(0);
        let end = integrity
            .chunk_range(index)
            .map_or(u64::MAX, |(_, end)| end);
        let iv: [u8; BLOCK] = integrity.iv.as_slice().try_into().unwrap_or([0_u8; BLOCK]);
        Self {
            integrity,
            cipher,
            index,
            end,
            state: iv,
            pending: [0_u8; BLOCK],
            pending_len: 0,
            position,
            exhausted: integrity.chunk_of(position).is_none(),
        }
    }

    /// Feeds plaintext written at `position`, returning every chunk MAC it completed.
    pub fn feed(
        &mut self,
        position: u64,
        plaintext: &[u8],
    ) -> Result<Vec<(usize, [u8; BLOCK])>, Failure> {
        if self.exhausted || plaintext.is_empty() {
            return Ok(Vec::new());
        }
        if position != self.position {
            return Err(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                CODE_CHECKPOINT_MISMATCH,
                format!(
                    "this stream must be accounted for in order: expected byte {}, got {position}",
                    self.position
                ),
            ));
        }
        let mut finished = Vec::new();
        let mut rest = plaintext;
        while !rest.is_empty() && !self.exhausted {
            let room = (self.end - self.position) as usize;
            let take = room.min(rest.len());
            self.absorb(&rest[..take]);
            self.position += take as u64;
            rest = &rest[take..];
            if self.position == self.end {
                finished.push((self.index, self.close()));
                self.advance();
            }
        }
        Ok(finished)
    }

    /// Where the next byte is expected.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    fn absorb(&mut self, mut bytes: &[u8]) {
        if self.pending_len > 0 {
            let take = (BLOCK - self.pending_len).min(bytes.len());
            self.pending[self.pending_len..self.pending_len + take].copy_from_slice(&bytes[..take]);
            self.pending_len += take;
            bytes = &bytes[take..];
            if self.pending_len == BLOCK {
                let block = self.pending;
                self.mix(&block);
                self.pending_len = 0;
            }
        }
        while bytes.len() >= BLOCK {
            let (block, rest) = bytes.split_at(BLOCK);
            let mut buffer = [0_u8; BLOCK];
            buffer.copy_from_slice(block);
            self.mix(&buffer);
            bytes = rest;
        }
        if !bytes.is_empty() {
            self.pending[..bytes.len()].copy_from_slice(bytes);
            self.pending_len = bytes.len();
        }
    }

    fn mix(&mut self, block: &[u8; BLOCK]) {
        for (state, byte) in self.state.iter_mut().zip(block.iter()) {
            *state ^= byte;
        }
        self.cipher.encrypt_block((&mut self.state).into());
    }

    /// Pads the tail with zeros and returns the chunk's MAC.
    fn close(&mut self) -> [u8; BLOCK] {
        if self.pending_len > 0 {
            for byte in self.pending.iter_mut().skip(self.pending_len) {
                *byte = 0;
            }
            let block = self.pending;
            self.mix(&block);
            self.pending_len = 0;
        }
        self.state
    }

    fn advance(&mut self) {
        self.index += 1;
        match self.integrity.chunk_range(self.index) {
            Some((_, end)) => {
                self.end = end;
                self.state = self
                    .integrity
                    .iv
                    .as_slice()
                    .try_into()
                    .unwrap_or([0_u8; BLOCK]);
            }
            None => self.exhausted = true,
        }
    }
}

/// One CBC-MAC over `data`, zero-padded to the block size.
fn cbc_mac(cipher: &Aes128, iv: &[u8; BLOCK], data: &[u8]) -> [u8; BLOCK] {
    let mut state = *iv;
    let mut offset = 0;
    while offset < data.len() {
        let mut block = [0_u8; BLOCK];
        let take = BLOCK.min(data.len() - offset);
        block[..take].copy_from_slice(&data[offset..offset + take]);
        for (byte, mixed) in state.iter_mut().zip(block.iter()) {
            *byte ^= mixed;
        }
        cipher.encrypt_block((&mut state).into());
        offset += BLOCK;
    }
    state
}

/// What a continuation runs, once the transform has had its say.
#[derive(Clone, Debug)]
pub struct ResumePlan {
    /// The chunk layout to run, already rewound to boundaries the MAC chain can start from.
    pub chunks: Vec<ChunkSpec>,
    /// The chunk MACs carried over from the previous attempt.
    pub macs: BTreeMap<usize, [u8; BLOCK]>,
    /// Whether parallel connections were given up to keep the chain computable.
    pub collapsed: bool,
    /// Whether the previous attempt's description was a different one, so nothing was kept.
    pub restarted: bool,
}

/// Decides what a continuation may keep.
///
/// Three rules, in order, and each of them is a way a naive resume computes something wrong:
///
/// 1. **A different description keeps nothing.** Its chunk MACs describe other plaintext.
/// 2. **Only a contiguous run of chunk MACs from a connection's start may be kept.** A gap
///    means some chunk in the middle was never accounted for, and the condensed value needs
///    all of them.
/// 3. **A connection may only start on a provider boundary.** Where the persisted layout does
///    not, the run falls back to a single stream from zero rather than MACing a chunk it has
///    only part of.
#[must_use]
pub fn plan_resume(
    transform: &StreamTransform,
    chunks: Vec<ChunkSpec>,
    checkpoint: &TransformCheckpoint,
    total: Option<u64>,
) -> ResumePlan {
    let restarted = !chunks.is_empty()
        && checkpoint.fingerprint.is_some()
        && checkpoint.fingerprint.as_deref() != Some(transform.fingerprint());
    let adopted = transform.adopt(checkpoint);
    let Some(integrity) = transform.integrity() else {
        // Without an integrity value nothing is stateful: CTR alone is seekable, so every
        // chunk layout is as good as any other and nothing is rewound.
        return ResumePlan {
            chunks,
            macs: BTreeMap::new(),
            collapsed: false,
            restarted,
        };
    };
    let aligned = chunks
        .iter()
        .all(|chunk| integrity.is_boundary(chunk.start));
    if !aligned {
        let mut single = chunks.into_iter().next().unwrap_or_else(|| ChunkSpec {
            id: rd_core::ChunkId::new(),
            start: 0,
            end: total,
            committed: 0,
        });
        single.start = 0;
        single.end = total.or_else(|| integrity.boundaries.last().copied());
        single.committed = 0;
        return ResumePlan {
            chunks: vec![single],
            macs: BTreeMap::new(),
            collapsed: true,
            restarted,
        };
    }
    let mut kept = BTreeMap::new();
    let chunks = chunks
        .into_iter()
        .map(|mut chunk| {
            let first = integrity.chunk_of(chunk.start).unwrap_or(usize::MAX);
            let mut index = first;
            let mut reached = chunk.start;
            while index != usize::MAX
                && let Some(mac) = adopted.get(&index)
                && let Some((_, end)) = integrity.chunk_range(index)
                && end <= integrity.aligned_floor(chunk.committed)
            {
                kept.insert(index, *mac);
                reached = end;
                index += 1;
            }
            chunk.committed = reached.max(chunk.start);
            chunk
        })
        .collect();
    ResumePlan {
        chunks,
        macs: kept,
        collapsed: false,
        restarted,
    }
}
