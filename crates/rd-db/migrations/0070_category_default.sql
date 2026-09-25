-- Exactly one category is the default.
--
-- `0052` gave `storage_roots` this invariant and left `categories` with the same defect it
-- describes: the flag was only ever cleared on other rows when an incoming category asked to be
-- the default. An install could therefore end up with none at all -- the first category created
-- without ticking the switch, or the default being deleted -- and routing then had no fallback
-- for a link that matched no rule.

-- Repair: keep the alphabetically first of several defaults.
UPDATE categories
   SET is_default = 0
 WHERE is_default = 1
   AND id != (SELECT id FROM categories WHERE is_default = 1 ORDER BY name LIMIT 1);

-- Repair: promote the alphabetically first category when the table has no default at all.
UPDATE categories
   SET is_default = 1
 WHERE (SELECT COUNT(*) FROM categories WHERE is_default = 1) = 0
   AND id = (SELECT id FROM categories ORDER BY name LIMIT 1);

-- "At most one" is structural from here on. "At least one" cannot be expressed as a constraint
-- and is maintained by the create, update and delete paths in config_store.rs.
CREATE UNIQUE INDEX IF NOT EXISTS idx_categories_single_default
    ON categories (is_default) WHERE is_default = 1;
