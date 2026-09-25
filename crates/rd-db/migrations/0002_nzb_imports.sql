CREATE TABLE nzb_imports (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    segment_count INTEGER NOT NULL CHECK(segment_count >= 0),
    total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE nzb_files (
    id TEXT PRIMARY KEY NOT NULL,
    import_id TEXT NOT NULL REFERENCES nzb_imports(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    poster TEXT NOT NULL,
    groups_json TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
    ordinal INTEGER NOT NULL,
    UNIQUE(import_id, ordinal)
);

CREATE TABLE nzb_segments (
    id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL REFERENCES nzb_files(id) ON DELETE CASCADE,
    number INTEGER NOT NULL CHECK(number > 0),
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    message_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued',
    server_attempts INTEGER NOT NULL DEFAULT 0,
    crc32 INTEGER,
    UNIQUE(file_id, number)
);

CREATE INDEX nzb_imports_state_idx ON nzb_imports(state, created_at);
CREATE INDEX nzb_segments_state_idx ON nzb_segments(state, file_id);
