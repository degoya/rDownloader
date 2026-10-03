-- RD-190-19: operational notifications come from checks that repeat (the update check, the
-- plugin repository refresh, an account check). One row per delivery a notice was queued for,
-- under that delivery's idempotency key, so the same notice reaches a rule once -- after a
-- restart, after the history was cleared and after the per-rule trim dropped the delivery.
CREATE TABLE notification_notices (
    idempotency_key TEXT PRIMARY KEY NOT NULL,
    rule_id TEXT NOT NULL,
    event TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_notification_notices_created ON notification_notices(created_at);
