-- Where each site rule came from, and the newest signed rule file per signer (RD-1200-05).
--
-- `origin` is one of `signed` (the signed release file; `origin_signer` is the key whose
-- signature held and `origin_sequence` the file's sequence), `import` (an unsigned file or a
-- pasted export), `editor` (written or changed in the settings page), `mcp` (written through an
-- MCP tool) or `unknown`. Every row an earlier build wrote is `unknown`: until now nothing
-- recorded whether a rule came from the signed file, and a rule that matches a shipped id may
-- have been edited since, so guessing would claim a signature for bytes it never covered. A
-- word a later build writes is read as `unknown` too.
ALTER TABLE site_rules ADD COLUMN origin TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE site_rules ADD COLUMN origin_signer TEXT;
ALTER TABLE site_rules ADD COLUMN origin_sequence INTEGER;

-- The highest sequence the import accepted per signing key. A signed file with a lower one is
-- refused (`site_rules.sequence_older`), so an older file cannot bring back what a newer one
-- fixed; the same sequence again is accepted and leaves the row alone. Empty on an existing
-- installation: the first signed file imported after this migration sets the mark.
CREATE TABLE site_rule_pack_sequences (
    signer TEXT PRIMARY KEY,
    sequence INTEGER NOT NULL,
    accepted_at TEXT NOT NULL
);
