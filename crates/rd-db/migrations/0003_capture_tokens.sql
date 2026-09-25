CREATE TABLE capture_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    token_sha256 TEXT NOT NULL UNIQUE,
    scopes_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE INDEX capture_tokens_active_idx ON capture_tokens(token_sha256, revoked_at);
