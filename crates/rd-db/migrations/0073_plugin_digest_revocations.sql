-- Withdrawn plugin package digests (RD-108-31).
--
-- The second axis of the trust store next to `plugin_trusted_keys`: revoking a signing key
-- takes down every plugin its author ever signed, while a published-then-withdrawn version
-- has to be refused on its own. `digest` is `rd_plugin_host::package_digest` — the frozen
-- length-prefixed digest over the archive's members, the same payload the signature covers —
-- as 64 lowercase hex characters, never a hash of the `.rdplug` file.
--
-- The trust store itself is in memory, so without this table a restart forgot every
-- withdrawal. The rows are read once at start and replace the in-memory set.
--
-- `plugin_id`, `plugin_name` and `version` are context, not identity: the plugin manager has
-- to be able to name the withdrawn version without re-hashing every installed component on
-- every page load.
CREATE TABLE plugin_digest_revocations (
    digest TEXT PRIMARY KEY,
    plugin_id TEXT NULL,
    plugin_name TEXT NULL,
    version TEXT NULL,
    reason TEXT NULL,
    revoked_at TEXT NOT NULL
);
