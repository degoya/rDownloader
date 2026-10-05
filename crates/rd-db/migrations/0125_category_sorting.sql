-- Sort and rename templates for series and films per category (RD-1100-08): a JSON object
-- with `series`, `dated` and `movie`, each a template or absent. NULL means no sorting; there
-- is no global template to inherit.
ALTER TABLE categories ADD COLUMN sorting_json TEXT NULL;
