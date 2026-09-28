-- The history of storage work: verified moves and dedupe links (RD-150-02).
--
-- One row per file an operation touched. A row starts `running` and ends `completed` or
-- `failed` with a stable error code; a row still `running` when the service starts is marked
-- `interrupted`, and the move it belonged to is carried on from wherever the data verifiably
-- is. No foreign keys: the history is what explains a file that moved, and it has to outlive
-- the download and the package it was about. The writer keeps the newest rows and drops the
-- oldest beyond its cap.
CREATE TABLE storage_operations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN ('move', 'dedupe')),
    state TEXT NOT NULL CHECK (state IN ('running', 'completed', 'failed', 'interrupted')),
    package_id TEXT NULL,
    download_id TEXT NULL,
    source_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    size_bytes INTEGER NULL,
    verified_digest TEXT NULL,
    error_code TEXT NULL,
    error_message TEXT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NULL
);

CREATE INDEX storage_operations_state ON storage_operations (state);
