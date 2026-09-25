-- Plugin signing keys the user confirmed on first use (trust on first use).
--
-- A plugin package carries its author's public key; installing one signed by an unknown
-- key surfaces its fingerprint for confirmation, and the confirmed key is recorded here so
-- the package still verifies after a restart.
CREATE TABLE plugin_trusted_keys (
    key_id TEXT PRIMARY KEY,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    plugin_name TEXT NULL,
    confirmed_at TEXT NOT NULL
);
