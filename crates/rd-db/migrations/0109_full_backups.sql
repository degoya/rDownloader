-- Scheduled encrypted full backups (RD-160-01): where they go, when they run, which key seals
-- them, and what every run ended with.
--
-- The passphrase is stored nowhere. `key_ref` names the key derived from it once at setup, kept
-- in the secret store so a scheduled run needs no input; `key_salt` is the Argon2id salt it was
-- derived with, which every archive carries in its header so a restore can ask for the
-- passphrase again and derive the same key.

-- Where a backup is written. RD-160-01 knows one kind, `local` (a folder or a mounted NAS path,
-- `config_json` = {"path": "..."}); RD-160-02 adds object storage and rclone as further kinds
-- without a schema change, which is why the kind is not fenced by a CHECK here.
CREATE TABLE backup_destinations (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- The one backup configuration of this installation.
CREATE TABLE backup_config (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    -- Five-field cron expression, read in `timezone` (an IANA name).
    schedule TEXT NOT NULL DEFAULT '0 3 * * *',
    timezone TEXT NOT NULL DEFAULT 'UTC',
    destination_id TEXT REFERENCES backup_destinations(id) ON DELETE SET NULL,
    key_ref TEXT,
    key_salt TEXT,
    -- Hex prefix of the key's SHA-256, to tell two keys apart in the interface; not the key.
    key_fingerprint TEXT,
    key_set_at TEXT,
    -- When the schedule is next due. Persisted so a restart neither skips nor repeats a run.
    next_run_at TEXT,
    updated_at TEXT NOT NULL,
    CHECK ((key_ref IS NULL) = (key_salt IS NULL))
);

INSERT INTO backup_config (id, updated_at) VALUES (1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

-- One row per run, newest kept. A run that is still `running` when the process starts again
-- was interrupted; the start marks it so.
CREATE TABLE backup_runs (
    id TEXT PRIMARY KEY NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('scheduled', 'manual')),
    state TEXT NOT NULL CHECK (state IN ('running', 'succeeded', 'failed', 'interrupted')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    -- No foreign key: the history outlives a destination that is replaced.
    destination_id TEXT,
    destination TEXT,
    archive_name TEXT,
    size_bytes INTEGER,
    sha256 TEXT,
    -- The manifest's parts (name, kind, size, SHA-256) of a finished run, as JSON.
    parts_json TEXT,
    -- The stable code of a failed or interrupted run and its English detail.
    error_code TEXT,
    error_detail TEXT
);

CREATE INDEX backup_runs_started_idx ON backup_runs(started_at DESC);

-- At most one run at a time, enforced by the store rather than by a lock in one process.
CREATE UNIQUE INDEX backup_runs_one_running_idx ON backup_runs(state) WHERE state = 'running';
