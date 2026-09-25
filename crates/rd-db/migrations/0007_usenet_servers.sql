CREATE TABLE usenet_servers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
    tls INTEGER NOT NULL DEFAULT 1,
    username TEXT,
    password_ref TEXT,
    proxy_profile_id TEXT,
    priority INTEGER NOT NULL DEFAULT 100,
    max_connections INTEGER NOT NULL DEFAULT 8 CHECK(max_connections BETWEEN 1 AND 32),
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX usenet_servers_priority_idx ON usenet_servers(enabled, priority);
