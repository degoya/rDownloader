CREATE TABLE hotfolders (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    executor_json TEXT NOT NULL,
    path TEXT NOT NULL,
    recursive INTEGER NOT NULL DEFAULT 0,
    category_id TEXT REFERENCES categories(id),
    import_mode TEXT NOT NULL,
    processed_path TEXT NOT NULL,
    failed_path TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(executor_json, path)
);

CREATE INDEX hotfolders_enabled_idx ON hotfolders(enabled, name);
