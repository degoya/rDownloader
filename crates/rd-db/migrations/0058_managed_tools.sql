-- Managed external tools (RD-102-02): what this installation installed itself, and how far
-- the signed tool manifest has advanced.
--
-- The *active* version is not a column here. It is the `active.json` pointer next to the
-- version directories, so that activation stays one atomic rename on the same filesystem as
-- the thing it points at, and so a database that is restored from an older backup cannot
-- claim a version that is no longer on disk. This table is the history a rollback reads:
-- which versions were installed, when, and from where.
CREATE TABLE managed_tools (
    -- One of the closed set in `rd_tools::manifest::MANAGED_TOOLS`.
    name TEXT NOT NULL,
    -- The tool's own version string, and the directory name under `<data>/tools/<name>/`.
    version TEXT NOT NULL,
    installed_at TEXT NOT NULL,
    -- Where the bytes came from, so a user can see what an installation actually fetched.
    source_url TEXT NOT NULL,
    -- The hex SHA-256 that was verified before this version was promoted. Kept so a later
    -- audit can compare an installed version against the manifest that produced it.
    sha256 TEXT NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE INDEX managed_tools_installed_idx ON managed_tools(name, installed_at DESC);

-- The replay guard for the tool manifest.
--
-- `rd_sign::replay::check` refuses a manifest whose sequence is not higher than the highest
-- one already accepted — but only if the caller remembers that number across restarts. This
-- single row is that memory. Without it the freshness rule degrades to "first contact"
-- forever, and yesterday's genuine, genuinely signed manifest can be served back indefinitely.
CREATE TABLE tool_manifest_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    sequence INTEGER NOT NULL,
    issued_at TEXT NOT NULL,
    accepted_at TEXT NOT NULL
);
