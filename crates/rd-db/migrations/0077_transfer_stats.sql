-- Persistent transfer statistics (RD-110-01).
--
-- Two tables with the same five figures, kept for two different questions.
--
-- `transfer_stats` answers "what happened when": one row per hour, download kind and
-- provider, later folded into one row per day by the retention sweep and eventually deleted.
-- `bucket_start` is RFC 3339 in UTC at a fixed width (`2026-09-20T14:00:00Z`), so ordering
-- and range comparison are plain string comparison and never need a parse.
--
-- `transfer_totals` answers "how much altogether": one row per kind and provider that is
-- only ever added to and never pruned. The Prometheus counters read from it, which is what
-- keeps them monotonic across a restart and across the sweep that thins the first table.
--
-- `provider` is the provider id of the account a transfer used, or `direct` when it used
-- none. It is never a label, a user name, a host or an address: the label set has to stay
-- small and free of anything a person typed.
CREATE TABLE transfer_stats (
    resolution TEXT NOT NULL CHECK (resolution IN ('hour', 'day')),
    bucket_start TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    retries INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    seconds INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (resolution, bucket_start, kind, provider)
);

CREATE INDEX transfer_stats_by_start ON transfer_stats (bucket_start);

CREATE TABLE transfer_totals (
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    retries INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    seconds INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, provider)
);
