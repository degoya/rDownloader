-- Automation engine: definitions, their immutable versions, and the run history
-- (RD-090-04). A run holds the version it started with, so editing an automation never
-- changes the rules an in-flight run is being judged by.
CREATE TABLE automations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE automation_versions (
    id TEXT PRIMARY KEY,
    automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    trigger_kind TEXT NOT NULL,
    condition_json TEXT NOT NULL,
    actions_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (automation_id, version)
);

CREATE INDEX idx_automation_versions_automation
    ON automation_versions (automation_id, version DESC);

CREATE TABLE automation_runs (
    id TEXT PRIMARY KEY,
    automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    automation_version_id TEXT NOT NULL REFERENCES automation_versions(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL,
    -- The package the actions operate on, when the triggering event named one. Stored
    -- rather than re-derived: the event row is append-only history, and re-classifying it
    -- later would resolve against whatever the listing looks like by then.
    package_id TEXT,
    -- Derived from the version and the event, so replaying an event after a crash collides
    -- here instead of running the same automation twice.
    idempotency_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    action_index INTEGER NOT NULL DEFAULT 0,
    attempt INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    message TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

-- The sweep looks for runs that are due; this is the index it walks.
CREATE INDEX idx_automation_runs_due ON automation_runs (state, next_attempt_at);
CREATE INDEX idx_automation_runs_history ON automation_runs (automation_id, started_at DESC);
