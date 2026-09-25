-- Reusable bandwidth profiles with their weekly schedule and traffic budgets (RD-050-12).
-- Scope limits ride as JSON on the profile: they are always read and written together with
-- it, and never queried on their own.
CREATE TABLE bandwidth_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    download_bytes_per_second INTEGER,
    upload_bytes_per_second INTEGER,
    max_active_files INTEGER,
    daily_budget_bytes INTEGER,
    monthly_budget_bytes INTEGER,
    scopes_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- One weekly time window. `days` is a Monday-first bitmask; an `end_minute` below
-- `start_minute` wraps past midnight into the following day.
CREATE TABLE bandwidth_windows (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES bandwidth_profiles(id) ON DELETE CASCADE,
    days INTEGER NOT NULL,
    start_minute INTEGER NOT NULL,
    end_minute INTEGER NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_bandwidth_windows_profile ON bandwidth_windows(profile_id);

-- Counters keyed by the period they belong to, so a restart resumes the same period and a
-- DST change can neither duplicate nor lose one.
CREATE TABLE bandwidth_budget_counters (
    profile_id TEXT NOT NULL,
    period_kind TEXT NOT NULL,
    period_key TEXT NOT NULL,
    used_bytes INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, period_kind)
);
