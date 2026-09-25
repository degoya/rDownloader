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

#[cfg(test)]
mod tests {
    use super::plan_chunks;

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
}
