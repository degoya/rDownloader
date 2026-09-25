-- RD-080-08: planned livestream recordings and their occurrences.
--
-- The time zone is stored as an IANA name rather than an offset. An offset does not survive
-- daylight saving, so a weekly show at 20:00 would move by an hour twice a year and nobody
-- would notice until a recording started at the wrong time.
--
-- `stream_scheduled_runs` carries the idempotency that makes the whole thing restart-safe:
-- the UNIQUE index over (schedule_id, starts_at) means one occurrence produces one row,
-- however often the planner runs. That is also what stops the repeated hour when the clocks
-- go back from becoming two recordings of one broadcast.
CREATE TABLE stream_schedules (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL REFERENCES stream_channels(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    -- 'once' or 'weekly'.
    kind TEXT NOT NULL,
    -- Set for 'once': the absolute instant, because a one-off names an event, not a time.
    start_at TEXT NULL,
    -- Set for 'weekly': ISO weekdays as a comma-separated list, and minutes after local
    -- midnight.
    days TEXT NULL,
    start_minute INTEGER NULL,
    timezone TEXT NOT NULL,
    window_minutes INTEGER NOT NULL,
    lead_minutes INTEGER NOT NULL DEFAULT 0,
    trail_minutes INTEGER NOT NULL DEFAULT 0,
    replay_from_start INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX stream_schedules_channel_idx ON stream_schedules(channel_id, enabled);

CREATE TABLE stream_scheduled_runs (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL REFERENCES stream_schedules(id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    state TEXT NOT NULL,
    download_id TEXT NULL,
    -- Whether replay-from-start was actually available, not merely requested. Stored so the
    -- UI never claims a capability the provider did not offer.
    replay_used INTEGER NOT NULL DEFAULT 0,
    error TEXT NULL,
    created_at TEXT NOT NULL
);

-- One occurrence, one row. This is the guarantee, not a convention.
CREATE UNIQUE INDEX stream_scheduled_runs_occurrence_idx
    ON stream_scheduled_runs(schedule_id, starts_at);
CREATE INDEX stream_scheduled_runs_open_idx
    ON stream_scheduled_runs(state, starts_at);
