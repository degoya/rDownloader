-- Which door an audited action came through, and a call limit per API token (RD-1200-04).
--
-- `via` is a word of `rd_core::AuditChannel`: rest, mcp, capture, compat, internal. The table
-- is append-only (migration 0079), so a record written before this column existed is not
-- rewritten and reads as `rest`, the default.
ALTER TABLE audit_records ADD COLUMN via TEXT NOT NULL DEFAULT 'rest';
CREATE INDEX audit_records_via ON audit_records (via, id);

-- Calls a token may make per minute, REST and MCP together. NULL is no limit, which is what
-- every token issued before this column existed keeps.
ALTER TABLE capture_tokens ADD COLUMN calls_per_minute INTEGER NULL;
