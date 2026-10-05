-- RD-1100-04: the persistent download history. One row per package, written in the transaction
-- that gives the package its outcome, and kept after the package left the queue -- which is why
-- `package_id` is no foreign key. The sources are stored masked (no userinfo, no credential query
-- values, no fragment); no password of any kind is stored. `compat_hidden` is set when a SABnzbd
-- client deleted the item from its history: the client sees it gone, the native history keeps it.
CREATE TABLE download_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    category TEXT,
    destination TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    sources_json TEXT NOT NULL,
    outcome TEXT NOT NULL,
    error_code TEXT,
    error_params_json TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT NOT NULL,
    compat_hidden INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_download_history_finished ON download_history(finished_at);
