-- Exactly one storage root is the default.
--
-- Until now the flag was only ever cleared on other rows when an incoming root asked to be the
-- default. An install could therefore end up with none at all -- the first root created without
-- ticking the switch, or the default being deleted -- and destination resolution silently fell
-- back to whichever root sorted first alphabetically.

-- Repair: keep the alphabetically first of several defaults.
UPDATE storage_roots
   SET is_default = 0
 WHERE is_default = 1
   AND id != (SELECT id FROM storage_roots WHERE is_default = 1 ORDER BY name LIMIT 1);

-- Repair: promote the alphabetically first root when the table has no default at all.
UPDATE storage_roots
   SET is_default = 1
 WHERE (SELECT COUNT(*) FROM storage_roots WHERE is_default = 1) = 0
   AND id = (SELECT id FROM storage_roots ORDER BY name LIMIT 1);

-- "At most one" is structural from here on. "At least one" cannot be expressed as a constraint
-- and is maintained by the create, update and delete paths in config_store.rs.
CREATE UNIQUE INDEX IF NOT EXISTS idx_storage_roots_single_default
    ON storage_roots (is_default) WHERE is_default = 1;
