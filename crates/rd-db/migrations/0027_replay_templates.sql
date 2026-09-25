-- Authenticated request replay (RD-050-04).
--
-- Capture side: the encrypted body reference and the explicit consent are kept in their own
-- columns rather than inside `request_json`, so neither can ever be serialized into an API
-- or SSE payload by widening a struct. The reference is a `vault://` id; the plaintext body
-- only ever exists in the secret store.
ALTER TABLE link_candidates ADD COLUMN replay_body_ref TEXT;
ALTER TABLE link_candidates ADD COLUMN replay_consent_json TEXT;

-- Transfer side: the consented template of one download. Separate from `downloads` because
-- it has its own lifecycle (a refresh replaces the URL, consent carries its own hash) and
-- because its vault reference must be cleaned up when the download is deleted.
CREATE TABLE download_request_templates (
    download_id   TEXT PRIMARY KEY NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    template_json TEXT NOT NULL,
    body_ref      TEXT,
    consent_json  TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- Pre-resume refresh budget, deliberately separate from the one-shot
-- `resolver_refresh_count` of migration 0008: sharing that counter would let a pre-resume
-- refresh burn the single reactive retry a later genuine 401 needs. Windowed rather than
-- absolute, because a long download that is paused twice legitimately needs more than one.
ALTER TABLE downloads ADD COLUMN replay_refresh_count INTEGER NOT NULL DEFAULT 0
    CHECK(replay_refresh_count BETWEEN 0 AND 3);
ALTER TABLE downloads ADD COLUMN replay_refreshed_at TEXT;
