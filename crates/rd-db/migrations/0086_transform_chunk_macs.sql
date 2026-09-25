-- Finished provider-chunk MACs of a transformed stream (RD-103-02, ADR 0011).
--
-- A MEGA download is verified against the value the provider published, and that value is
-- condensed from one MAC per provider chunk. The MACs are accumulated while the bytes are
-- written, so a restart that kept the part file has to keep them too -- otherwise every
-- continuation would have to re-read what it already has just to recompute them.
--
-- `fingerprint` is `ContentTransform::fingerprint`: it says which description wrote these,
-- so a continuation under a different key or a different chunk layout starts over instead
-- of condensing somebody else's state.
CREATE TABLE transform_chunk_macs (
    download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK(chunk_index >= 0),
    mac BLOB NOT NULL,
    fingerprint TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (download_id, chunk_index)
);
