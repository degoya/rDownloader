use rd_core::ChunkId;

const MIN_CHUNK_SIZE: u64 = 4 * 1024 * 1024;

/// Inclusive-exclusive byte range with a durable resume position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkSpec {
    pub id: ChunkId,
    pub start: u64,
    pub end: Option<u64>,
    pub committed: u64,
}

impl ChunkSpec {
    /// Returns whether every byte in a bounded chunk has been committed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.end.is_some_and(|end| self.committed >= end)
    }
}

/// Divides a known file into balanced chunks when range requests are supported.
#[must_use]
pub fn plan_chunks(total: Option<u64>, accepts_ranges: bool, max_chunks: usize) -> Vec<ChunkSpec> {
    let Some(total) = total else {
        return vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: None,
            committed: 0,
        }];
    };
    if total == 0 {
        return vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(0),
            committed: 0,
        }];
    }
    let possible = total.div_ceil(MIN_CHUNK_SIZE) as usize;
    let count = if accepts_ranges {
        max_chunks.clamp(1, possible.max(1))
    } else {
        1
    };
    let chunk_size = total.div_ceil(count as u64);
    (0..count)
        .map(|index| {
            let start = index as u64 * chunk_size;
            let end = ((index as u64 + 1) * chunk_size).min(total);
            ChunkSpec {
                id: ChunkId::new(),
                start,
                end: Some(end),
                committed: start,
            }
        })
        .collect()
}

/// Divides a file of known size into chunks whose boundaries fall on multiples of `align`
/// (RD-150-03).
///
/// A mirror set with piece hashes verifies a chunk as soon as it is complete, which needs every
/// piece to lie inside exactly one chunk; `align` is the piece length then, and `1` without
/// pieces. The count is `max_chunks` as far as the file allows chunks of at least
/// [`MIN_CHUNK_SIZE`] and one piece.
#[must_use]
pub fn plan_aligned_chunks(total: u64, max_chunks: usize, align: u64) -> Vec<ChunkSpec> {
    let align = align.max(1);
    if total == 0 {
        return plan_chunks(Some(0), true, 1);
    }
    let floor = MIN_CHUNK_SIZE.max(align);
    let possible = usize::try_from(total.div_ceil(floor)).unwrap_or(usize::MAX);
    let count = max_chunks.clamp(1, possible.max(1)) as u64;
    let size = total
        .div_ceil(count)
        .div_ceil(align)
        .saturating_mul(align)
        .max(align);
    let mut chunks = Vec::new();
    let mut start = 0_u64;
    while start < total {
        let end = start.saturating_add(size).min(total);
        chunks.push(ChunkSpec {
            id: ChunkId::new(),
            start,
            end: Some(end),
            committed: start,
        });
        start = end;
    }
    chunks
}

/// Whether every boundary of `chunks` falls on a multiple of `align` or on the end of the file.
#[must_use]
pub fn chunks_aligned(chunks: &[ChunkSpec], align: u64, total: u64) -> bool {
    let align = align.max(1);
    chunks.iter().all(|chunk| {
        chunk.start % align == 0
            && chunk
                .end
                .is_some_and(|end| end % align == 0 || end == total)
    })
}

#[cfg(test)]
mod tests {
    use super::{MIN_CHUNK_SIZE, chunks_aligned, plan_aligned_chunks, plan_chunks};

    #[test]
    fn chunks_cover_file_without_overlap() {
        let chunks = plan_chunks(Some(20 * 1024 * 1024), true, 4);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks.first().map(|chunk| chunk.start), Some(0));
        assert_eq!(
            chunks.last().and_then(|chunk| chunk.end),
            Some(20 * 1024 * 1024)
        );
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end, Some(pair[1].start));
        }
    }

    #[test]
    fn aligned_chunks_fall_on_piece_boundaries_and_cover_the_file() {
        let piece = 3 * 1024 * 1024;
        let total = 20 * 1024 * 1024 + 17;
        let chunks = plan_aligned_chunks(total, 4, piece);
        assert!(chunks.len() > 1);
        assert!(chunks_aligned(&chunks, piece, total));
        assert_eq!(chunks.first().map(|chunk| chunk.start), Some(0));
        assert_eq!(chunks.last().and_then(|chunk| chunk.end), Some(total));
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end, Some(pair[1].start));
        }
        // A file below two minimum chunks is one chunk, whatever was asked for.
        assert_eq!(plan_aligned_chunks(MIN_CHUNK_SIZE, 8, 1).len(), 1);
        assert!(!chunks_aligned(
            &plan_chunks(Some(total), true, 4),
            piece,
            total
        ));
    }
}
