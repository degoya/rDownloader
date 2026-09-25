-- Login sessions, and when a machine token was last used.
--
-- Sessions were a HashMap in the process: they vanished on restart, carried no metadata, and
-- could not be listed or revoked. A person who wants to know what is signed in to their
-- service, and to sign something else out, had no way to do either.
--
-- The bearer itself is never stored, only its SHA-256 digest — the same rule the capture
-- tokens follow. A database that is read (a backup, a stolen file) must not hand over live
-- credentials.
CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    token_sha256 TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    -- What asked for the session, truncated. Present so a person can recognise their own
    -- devices in the list; absent when the client sent no user agent.
    user_agent TEXT,
    -- The address the session was created from, as the trusted-proxy rules resolved it.
    -- Local to this installation and deliberately kept out of logs and diagnostics.
    client_ip TEXT,
    revoked_at TEXT
);

CREATE INDEX sessions_active_idx ON sessions(token_sha256, revoked_at);
CREATE INDEX sessions_expiry_idx ON sessions(expires_at);

-- The same question for machine tokens: an inventory that cannot say when a token was last
-- used cannot tell a live integration from one nobody has revoked yet.
ALTER TABLE capture_tokens ADD COLUMN last_used_at TEXT;
