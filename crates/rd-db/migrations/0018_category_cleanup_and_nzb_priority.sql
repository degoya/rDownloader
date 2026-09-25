-- Per-category cleanup extensions (JSON array; NULL inherits the global setting).
ALTER TABLE categories ADD COLUMN cleanup_extensions TEXT NULL;

-- Priority chosen at NZB import time; NULL keeps the caller's default.
ALTER TABLE nzb_imports ADD COLUMN priority INTEGER NULL;
