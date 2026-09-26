-- Signed plugin repositories (RD-140-01): where plugin packages are offered, how far each
-- repository's signed index has advanced, which signing keys a repository withdrew, and which
-- installed versions came from which repository.
--
-- A repository only delivers. Every package it lists still verifies under a trusted *plugin*
-- key on install; the repository key below proves only that the index came from whoever the
-- person approved.
CREATE TABLE plugin_repositories (
    -- `official` for the built-in repository, a UUID for every other one.
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('official', 'third_party')),
    name TEXT NOT NULL,
    -- The index's https:// address. NULL for the official repository, whose address and key
    -- are compiled in, so a database restored into a newer build never pins an old one.
    url TEXT,
    -- The approved repository key: the id the index's signature names, the Base64 Ed25519 key
    -- the person pasted, and its hex SHA-256 as it was shown when they approved it. NULL for
    -- the official repository, which verifies against the compiled-in `Role::Repository` root.
    key_id TEXT,
    public_key TEXT,
    fingerprint TEXT,
    -- A disabled repository is neither refreshed nor offered; what was installed from it stays.
    enabled INTEGER NOT NULL DEFAULT 1,
    -- The replay floor: the highest index sequence accepted from this repository. An index at or
    -- below it is refused, so a replayed older index cannot take back an update or a withdrawal.
    sequence INTEGER,
    issued_at TEXT,
    last_checked_at TEXT,
    last_success_at TEXT,
    -- The stable code of the last refresh's failure, NULL after a successful one.
    last_error TEXT,
    created_at TEXT NOT NULL,
    CHECK ((kind = 'official') = (url IS NULL)),
    CHECK (
        kind = 'official'
        OR (key_id IS NOT NULL AND public_key IS NOT NULL AND fingerprint IS NOT NULL)
    )
);

CREATE UNIQUE INDEX plugin_repositories_url_idx ON plugin_repositories(url) WHERE url IS NOT NULL;

INSERT INTO plugin_repositories (id, kind, name, enabled, created_at)
VALUES ('official', 'official', 'rDownloader', 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

-- Plugin signing keys a repository index withdrew, by fingerprint. Kept after the repository
-- is removed and after a later index stops naming the key: a withdrawal is not undone by
-- silence, and the verifier refuses every package signed by one of these, trusted or not.
CREATE TABLE plugin_withdrawn_keys (
    fingerprint TEXT PRIMARY KEY,
    key_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    withdrawn_at TEXT NOT NULL
);

-- Which installed version came from which repository. A third-party repository may withdraw
-- only what it delivered, and this is what "delivered" is measured against.
CREATE TABLE plugin_repository_installs (
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    -- `package_digest` in 64 lowercase hex characters, as the index named it.
    digest TEXT NOT NULL,
    repository_id TEXT NOT NULL REFERENCES plugin_repositories(id) ON DELETE CASCADE,
    installed_at TEXT NOT NULL,
    PRIMARY KEY (plugin_id, version)
);
