-- The audit log (RD-110-03).
--
-- One row per security-relevant action: a sign-in, a token, a configuration change, a plugin
-- trust decision, a destructive action. Separate from `log_records` on purpose, and the
-- separation is the point rather than tidiness:
--
--   * A log record may be dropped. The capture layer hands records to a bounded channel with
--     `try_send` and counts what it could not fit, because a download engine must not stall
--     behind the database. An audit record is written through the serialized writer and
--     awaited by the action that caused it, so the action cannot be reported done without it.
--   * Retention differs by an order of magnitude. A diagnostic log is two weeks; the question
--     an audit log answers is asked months later.
--   * A log record is free-form text. These columns are a closed vocabulary
--     (`rd_core::AuditAction`, `AuditOutcome`, `AuditActorKind`) that a filter can be written
--     against and a test can enumerate.
--
-- **Append-only.** The trigger below aborts every UPDATE, whatever issues it: there is no
-- writer command that updates a row, no REST route that edits one, and now no path at all.
-- DELETE stays possible because retention has to work, and it is the only thing that deletes:
-- `prune_audit_records` removes whole rows by `id`, oldest first, in bounded batches. Nothing
-- in the domain — deleting a download, a package, a category, a storage root, restoring a
-- backup — touches this table, and `backup_store::replace_all` does not list it.
--
-- Every text column already went through `rd_core::redact_text` before it arrived, and no
-- column ever holds a password, a token, a token digest or a signed URL.
CREATE TABLE audit_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at TEXT NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    -- An opaque handle for the actor: a token id, a session handle. Never a credential and
    -- never a digest that could be replayed.
    actor_id TEXT,
    actor_label TEXT,
    -- The address the action came from, when a request carried one. The point of a login
    -- record, and the reason this table is `api:admin` and not `api:read`.
    client_address TEXT,
    target_kind TEXT,
    target_id TEXT,
    target_name TEXT,
    -- The trace this action belongs to (`rd_core::TraceContext`), so an audit record and the
    -- log records of the same request are one query apart.
    trace_id TEXT,
    -- Whatever else the action wants to say, as a flat object of strings. Redacted like the
    -- rest; the store does not read it.
    details_json TEXT
);

CREATE INDEX audit_records_recorded_at ON audit_records (recorded_at);
CREATE INDEX audit_records_action ON audit_records (action, id);
CREATE INDEX audit_records_actor ON audit_records (actor_kind, actor_id);
CREATE INDEX audit_records_trace ON audit_records (trace_id);

CREATE TRIGGER audit_records_are_append_only
BEFORE UPDATE ON audit_records
BEGIN
    SELECT RAISE(ABORT, 'audit_records is append-only');
END;
