-- Checkpoints of plugin transfer backends (RD-070-02).
--
-- The checkpoint is opaque: only the backend that wrote it can read it, so the host stores
-- the bytes and nothing else. `plugin_version` is the pin — while a job is running it is
-- resumed by exactly the version that produced its checkpoint, because a newer build's
-- checkpoint format is its own business. An upgrade takes effect the next time the job starts.
CREATE TABLE plugin_transfers (
    download_id TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    plugin_id TEXT NOT NULL,
    plugin_version TEXT NOT NULL,
    checkpoint BLOB,
    updated_at TEXT NOT NULL
);
