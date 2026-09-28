-- Collision policies, collision prompts and the content index (RD-150-01).
--
-- `collision_policies` holds the policy of a category or a package when it has one of its own;
-- the global one is `storage_collision_policy` in the settings document, so a row's absence
-- means "inherit". Deliberately without a foreign key: a restored configuration backup deletes
-- and reinserts every category under its old id, and a cascade would drop their policies on the
-- way. A row whose category or package is gone matches nothing and is filtered out on read.
CREATE TABLE collision_policies (
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('category', 'package')),
    scope_id TEXT NOT NULL,
    policy TEXT NOT NULL CHECK (policy IN ('rename', 'skip', 'overwrite', 'compare', 'ask')),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope_kind, scope_id)
);

-- One open question per download: its file collided under the `ask` policy, and the download
-- sits in `blocked` with `block_reason = 'collision-ask'` until somebody answers. The row is
-- what makes the question survive a restart; the answer is kept here until the next attempt
-- carries it out and removes the row.
CREATE TABLE collision_prompts (
    download_id TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    target_name TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('before_transfer', 'after_transfer')),
    existing_bytes INTEGER NULL,
    decision TEXT NULL CHECK (decision IN ('rename', 'skip', 'overwrite')),
    created_at TEXT NOT NULL,
    decided_at TEXT NULL
);

-- The content hash of every finished file whose digest is known, and where that file lies.
-- Keyed by the download, so removing the download removes its entry; a move rewrites `path`,
-- and a file that is no longer where the entry says is marked `missing_since` rather than
-- forgotten, so it is found again if it comes back.
CREATE TABLE content_index (
    download_id TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    algorithm TEXT NOT NULL,
    digest TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    path TEXT NOT NULL,
    indexed_at TEXT NOT NULL,
    missing_since TEXT NULL
);

CREATE INDEX content_index_digest ON content_index (algorithm, digest);
