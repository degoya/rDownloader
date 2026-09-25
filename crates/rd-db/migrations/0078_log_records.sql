-- The structured log store (RD-110-02).
--
-- One row per tracing event the capture layer let through, written in batches by the
-- serialized writer and read by the log viewer and the diagnostic bundle. Every column
-- already went through `rd_core::redact_text` before it arrived here: this table never
-- holds a credential, a signed URL parameter or an authorization header, and a person
-- who copies the database file copies nothing they could not have read in the viewer.
--
-- `fields_json` is the event's other fields as a flat object of strings; the store does not
-- read it. `correlation_id` is whichever of the correlation fields the layer found first, so
-- one index answers "everything about this download". Retention deletes by `id` in bounded
-- batches, which is why `id` is the insertion order and nothing else.
CREATE TABLE log_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at TEXT NOT NULL,
    level TEXT NOT NULL,
    component TEXT NOT NULL,
    code TEXT,
    correlation_id TEXT,
    message TEXT NOT NULL,
    fields_json TEXT
);

CREATE INDEX log_records_recorded_at ON log_records (recorded_at);
CREATE INDEX log_records_level ON log_records (level, id);
CREATE INDEX log_records_correlation ON log_records (correlation_id);
