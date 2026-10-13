//! What a continuation of a transformed download may keep: the chunk MACs, the chunk layout
//! and the parallelism (RD-110-33). Split from `transform.rs`, which re-exports it, so neither
//! file outgrows the 500-line rule (RD-1280-08).

use std::collections::BTreeMap;

use crate::ChunkSpec;

use super::{BLOCK, StreamTransform, TransformCheckpoint};

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
