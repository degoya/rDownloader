-- RD-080-07: subscriptions, their poll history, and the item archive behind both.
--
-- The archive is `subscription_items`, and the UNIQUE index on (subscription_id, item_key)
-- is the whole once-only guarantee. "This item was already handled" is then a database fact
-- rather than a check somebody has to remember to perform, which is what makes it survive a
-- restart and a feed that reorders itself.
--
-- A rejected item is stored too, with the rule that rejected it. Two reasons: the next poll
-- must not reconsider it, and "nothing appeared" has to be distinguishable from a filter
-- that is quietly rejecting everything.
CREATE TABLE subscriptions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    kind TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    mode TEXT NOT NULL,
    category_id TEXT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    interval_seconds INTEGER NOT NULL,
    filters_json TEXT NULL,
    backlog_json TEXT NULL,
    -- Whether the first poll has happened. Until it has, the backlog policy applies; after
    -- it, everything the source offers is genuinely new.
    primed INTEGER NOT NULL DEFAULT 0,
    last_run_at TEXT NULL,
    next_run_at TEXT NULL,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_error TEXT NULL,
    -- HTTP cache validators for feed sources (RD-080-10).
    etag TEXT NULL,
    last_modified TEXT NULL,
    -- Opaque vault reference for an indexer API key (RD-080-11); never the key itself.
    secret_ref TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX subscriptions_due_idx ON subscriptions(enabled, next_run_at);

CREATE TABLE subscription_items (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    published_at TEXT NULL,
    duration_seconds INTEGER NULL,
    state TEXT NOT NULL,
    reason TEXT NULL,
    discovered_at TEXT NOT NULL
);

-- The once-only guarantee, expressed where it cannot be forgotten.
CREATE UNIQUE INDEX subscription_items_key_idx
    ON subscription_items(subscription_id, item_key);
CREATE INDEX subscription_items_state_idx
    ON subscription_items(subscription_id, state, discovered_at);

CREATE TABLE subscription_runs (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    finished_at TEXT NULL,
    found INTEGER NOT NULL DEFAULT 0,
    accepted INTEGER NOT NULL DEFAULT 0,
    skipped INTEGER NOT NULL DEFAULT 0,
    error TEXT NULL
);

CREATE INDEX subscription_runs_recent_idx
    ON subscription_runs(subscription_id, started_at DESC);
