-- Reusable per-domain session and authentication profiles (RD-050-03).
-- Credentials live in the secret store; only opaque vault:// references are kept here.
CREATE TABLE auth_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    include_subdomains INTEGER NOT NULL DEFAULT 0,
    path_prefix TEXT,
    method TEXT NOT NULL,
    origin TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    expires_at TEXT,
    username TEXT,
    secret_ref TEXT,
    certificate_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- One profile per distinct scope, so auto-matching never has to break a tie between
-- two profiles that cover exactly the same URLs.
CREATE UNIQUE INDEX auth_profiles_scope_idx
    ON auth_profiles(host, include_subdomains, IFNULL(path_prefix, ''));
CREATE INDEX auth_profiles_host_idx ON auth_profiles(host);

-- Per-job selection. Two columns because "let the scope decide" (auto) and "send nothing"
-- are different intents that a single nullable id cannot express.
ALTER TABLE downloads ADD COLUMN auth_profile_id TEXT;
ALTER TABLE downloads ADD COLUMN auth_profile_pinned INTEGER NOT NULL DEFAULT 0;

CREATE INDEX downloads_auth_profile_idx ON downloads(auth_profile_id);
