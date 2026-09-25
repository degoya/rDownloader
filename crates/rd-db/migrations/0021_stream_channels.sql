-- Livestream channels watched by the recording monitor.
CREATE TABLE stream_channels (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    name TEXT NOT NULL,
    quality TEXT NULL,
    category_id TEXT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_live_at TEXT NULL,
    last_error TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
