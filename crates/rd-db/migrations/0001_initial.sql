PRAGMA foreign_keys = ON;

CREATE TABLE packages (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    state TEXT NOT NULL,
    destination TEXT NOT NULL,
    category_id TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE downloads (
    id TEXT PRIMARY KEY NOT NULL,
    package_id TEXT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    source_url TEXT NOT NULL,
    file_name TEXT NOT NULL,
    state TEXT NOT NULL,
    total_bytes INTEGER,
    committed_bytes INTEGER NOT NULL DEFAULT 0 CHECK(committed_bytes >= 0),
    checksum_algorithm TEXT,
    checksum_value TEXT,
    computed_checksum_algorithm TEXT,
    computed_checksum_value TEXT,
    etag TEXT,
    last_modified TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    resolver_route_json TEXT,
    last_error_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX downloads_package_idx ON downloads(package_id);
CREATE INDEX downloads_state_idx ON downloads(state, created_at);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY NOT NULL,
    download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    start_offset INTEGER NOT NULL CHECK(start_offset >= 0),
    end_offset INTEGER,
    committed_offset INTEGER NOT NULL CHECK(committed_offset >= start_offset),
    updated_at TEXT NOT NULL,
    UNIQUE(download_id, start_offset)
);

CREATE TABLE accounts (
    id TEXT PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    username TEXT,
    secret_ref TEXT,
    cookie_ref TEXT,
    proxy_profile_id TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE proxy_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    username TEXT,
    secret_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE events (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX events_occurred_idx ON events(occurred_at);

CREATE TABLE storage_roots (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL UNIQUE,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE categories (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    color TEXT NOT NULL,
    storage_root_id TEXT NOT NULL REFERENCES storage_roots(id),
    relative_path TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE category_rules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    priority INTEGER NOT NULL,
    source TEXT,
    domain TEXT,
    protocol TEXT,
    extension TEXT,
    mime_type TEXT,
    name_regex TEXT,
    category_id TEXT NOT NULL REFERENCES categories(id),
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX category_rules_priority_idx ON category_rules(priority);

CREATE TABLE collector_batches (
    id TEXT PRIMARY KEY NOT NULL,
    source TEXT NOT NULL,
    source_label TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE link_candidates (
    id TEXT PRIMARY KEY NOT NULL,
    batch_id TEXT NOT NULL REFERENCES collector_batches(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    state TEXT NOT NULL,
    file_name TEXT,
    size INTEGER,
    provider TEXT,
    category_id TEXT REFERENCES categories(id),
    route_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX link_candidates_batch_idx ON link_candidates(batch_id);
CREATE INDEX link_candidates_url_idx ON link_candidates(url);
