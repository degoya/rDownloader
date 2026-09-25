-- Per-category seeding override (RD-050-11): enabled, ratio and seed time, each of which
-- inherits from the global settings when unset. NULL keeps the whole category inheriting.
ALTER TABLE categories ADD COLUMN seeding_json TEXT;
