-- Notification targets, the rules that route events to them and the delivery history
-- (RD-050-14). Secrets live in the encrypted vault; only the reference is stored here.
CREATE TABLE notification_targets (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    endpoint TEXT NOT NULL,
    config_json TEXT NOT NULL DEFAULT '{}',
    secret_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE notification_rules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    target_id TEXT NOT NULL REFERENCES notification_targets(id),
    events_json TEXT NOT NULL DEFAULT '[]',
    category_id TEXT,
    min_severity TEXT NOT NULL DEFAULT 'info',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_notification_rules_target ON notification_rules(target_id);

-- One row per (rule, event). The unique idempotency key is what guarantees that an event
-- produces at most one delivery per matching rule, even if the worker restarts mid-flight.
CREATE TABLE notification_deliveries (
    id TEXT PRIMARY KEY NOT NULL,
    rule_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    event TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued',
    attempt INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    response_status INTEGER,
    response_excerpt TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_notification_deliveries_pending
    ON notification_deliveries(state, next_attempt_at);
