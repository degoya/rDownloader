-- Several backup destinations, their retention, the ledger of written archives and their
-- verification (RD-160-02).
--
-- Every row of `backup_destinations` is now a destination of the schedule, each receiving its
-- own copy of every archive; `backup_config.destination_id` of 0109 is no longer read.
-- The kinds are `local` ({"path": ...}), `object_storage` ({"profile_id": ..., "prefix":
-- "<bucket>/<folder>"}) and `rclone` ({"remote": "name:path"}).

-- Retention per destination: the newest `keep_last` archives and those younger than
-- `keep_days` stay; NULL is no limit of that kind, and the newest archive always stays.
ALTER TABLE backup_destinations ADD COLUMN keep_last INTEGER
    CHECK (keep_last IS NULL OR keep_last >= 1);
ALTER TABLE backup_destinations ADD COLUMN keep_days INTEGER
    CHECK (keep_days IS NULL OR keep_days >= 1);

-- This installation's id, part of every archive name, so retention recognises its own archives
-- in a folder another installation writes to as well. Eight hex characters, made once here.
ALTER TABLE backup_config ADD COLUMN instance_id TEXT;
UPDATE backup_config SET instance_id = lower(hex(randomblob(4))) WHERE id = 1;

-- The scheduled verification: a five-field cron expression read in `backup_config.timezone`
-- (NULL = off), and when it is next due, persisted like `next_run_at`.
ALTER TABLE backup_config ADD COLUMN verify_schedule TEXT;
ALTER TABLE backup_config ADD COLUMN verify_next_run_at TEXT;

-- Every archive this installation placed at a destination. Retention deletes only what is
-- listed here, and verification compares against the size and SHA-256 recorded here; a file
-- at a destination without a row is never touched. Removing a destination forgets its rows
-- and leaves its archives where they are.
CREATE TABLE backup_archives (
    id TEXT PRIMARY KEY NOT NULL,
    destination_id TEXT NOT NULL REFERENCES backup_destinations(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    archive_name TEXT NOT NULL,
    location TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    -- When the run that wrote it started: the archive's point in time.
    created_at TEXT NOT NULL,
    stored_at TEXT NOT NULL,
    verified_at TEXT,
    verify_state TEXT CHECK (verify_state IS NULL OR verify_state IN ('passed', 'failed')),
    verify_code TEXT,
    UNIQUE (destination_id, archive_name)
);

CREATE INDEX backup_archives_destination_idx ON backup_archives(destination_id, created_at DESC);

-- How each destination of a run fared. No foreign key to the destination: the history
-- outlives a destination that is removed.
CREATE TABLE backup_run_destinations (
    run_id TEXT NOT NULL REFERENCES backup_runs(id) ON DELETE CASCADE,
    destination_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    destination TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('running', 'succeeded', 'failed', 'interrupted')),
    attempts INTEGER NOT NULL DEFAULT 0,
    location TEXT,
    -- How many older archives retention removed there after this run.
    pruned INTEGER NOT NULL DEFAULT 0,
    error_code TEXT,
    error_detail TEXT,
    finished_at TEXT,
    PRIMARY KEY (run_id, destination_id)
);

-- One row per verification of one archive, newest kept.
CREATE TABLE backup_verifications (
    id TEXT PRIMARY KEY NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('scheduled', 'manual')),
    state TEXT NOT NULL CHECK (state IN ('running', 'passed', 'failed', 'interrupted')),
    archive_id TEXT,
    destination_id TEXT,
    destination TEXT NOT NULL,
    archive_name TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    -- Whether the archive was opened and its members checked, or only its digest compared
    -- (an archive sealed under an earlier passphrase).
    content_checked INTEGER CHECK (content_checked IS NULL OR content_checked IN (0, 1)),
    error_code TEXT,
    error_detail TEXT
);

CREATE INDEX backup_verifications_started_idx ON backup_verifications(started_at DESC);
