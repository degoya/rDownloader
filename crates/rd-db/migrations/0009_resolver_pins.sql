CREATE TABLE download_resolver_pins (
    download_id TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    plugin_id TEXT NOT NULL,
    plugin_version TEXT NOT NULL,
    created_at TEXT NOT NULL
);
