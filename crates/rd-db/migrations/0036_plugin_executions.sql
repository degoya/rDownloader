-- Sanitised execution history of plugin invocations (RD-070-03).
--
-- What a crashing plugin looks like from the outside is "the download failed again", and the
-- reason lived only in a log line nobody kept. This keeps the classification — which plugin,
-- which version, which failure class. `message` passes the same redaction every persisted
-- failure passes, so a credential or a session token in a resolver's error text never gets
-- here; the host name does, because without it the entry says nothing.
--
-- `correlation_id` is a bare UUID with no relation to the request, so a user can quote it in a
-- report and it can be found again without it identifying anything by itself.
CREATE TABLE plugin_executions (
    id TEXT PRIMARY KEY NOT NULL,
    plugin_id TEXT NOT NULL,
    plugin_version TEXT NOT NULL,
    plugin_type TEXT NOT NULL,
    operation TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    error_class TEXT,
    message TEXT,
    started_at TEXT NOT NULL,
    duration_ms INTEGER NOT NULL
);

-- The history is read per plugin, newest first, and trimmed the same way.
CREATE INDEX plugin_executions_plugin_idx
    ON plugin_executions(plugin_id, started_at DESC);
