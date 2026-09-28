-- Several sources for one file (RD-150-03): a Metalink document's mirrors, ranked, with the
-- hashes their bytes must match, fetched by one transfer that takes chunks from several of
-- them at once.
--
-- `position` is the order the sources are tried in. It is fixed when the download is created
-- (priority first, then the order the document gave) and never recomputed, so a restart walks
-- the sources exactly as the attempt before it did. The health columns are what a failover
-- decides on and what survives a restart: a source that failed waits until `backoff_until`, a
-- source that delivered wrong bytes carries `isolated_code` and is not tried again.
CREATE TABLE download_sources (
    download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    url TEXT NOT NULL,
    protocol TEXT NOT NULL CHECK (protocol IN ('http', 'https', 'ftp', 'ftps', 'sftp')),
    priority INTEGER,
    -- ISO 3166-1 alpha-2, lowercase, as the document gave it.
    location TEXT,
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    backoff_until TEXT,
    isolated_code TEXT,
    last_error_code TEXT,
    delivered_bytes INTEGER NOT NULL DEFAULT 0 CHECK (delivered_bytes >= 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (download_id, position)
);

-- The piece hashes of a download with a source set, one JSON array of lowercase hex. A row
-- only when the document stated pieces that cover the file exactly; the whole-file hash goes
-- into the existing `downloads.checksum_*` columns and is verified before promotion as always.
CREATE TABLE download_piece_hashes (
    download_id TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    algorithm TEXT NOT NULL,
    piece_length INTEGER NOT NULL CHECK (piece_length > 0),
    hashes_json TEXT NOT NULL
);

-- Which source delivered a chunk's bytes, and whether its pieces were checked. A chunk that is
-- complete and unverified after a restart is checked before anything builds on it; a mismatch
-- isolates the source named here.
ALTER TABLE chunks ADD COLUMN source_position INTEGER;
ALTER TABLE chunks ADD COLUMN verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1));

-- The checked source set a LinkGrabber candidate carries until it is queued. JSON of
-- `rd_core::SourceSet`; NULL for every link that did not come with one.
ALTER TABLE link_candidates ADD COLUMN source_set_json TEXT;
