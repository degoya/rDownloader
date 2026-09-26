-- Which installed version of a plugin runs, and which one is under test (RD-140-02).
--
-- Installing never removes an older version, and until 1.4 the highest SemVer always won. A
-- row here points one plugin at a chosen version instead: `active_version` is what new work
-- runs on from the next start (a rollback writes the version it rolls back to),
-- `previous_version` is what a rollback returns to, and `staged_version` is a version under
-- test that only a download pinned to it explicitly runs. A plugin without a row keeps the
-- old rule, so nothing changes until the operator acts.
--
-- The versions are pointers, not guarantees: a pointer at a version that is removed or
-- withdrawn simply does not match anything that loads, and the newest loaded version that is
-- not under test takes over. `update_policy` is the operator's choice per plugin between
-- installing an offered update by hand (`manual`) or on its own (`automatic`).
CREATE TABLE plugin_version_choices (
    plugin_id TEXT PRIMARY KEY NOT NULL,
    active_version TEXT NULL,
    previous_version TEXT NULL,
    staged_version TEXT NULL,
    update_policy TEXT NOT NULL DEFAULT 'manual'
        CHECK (update_policy IN ('manual', 'automatic')),
    updated_at TEXT NOT NULL
);
