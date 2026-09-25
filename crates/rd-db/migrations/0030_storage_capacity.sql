-- Per-root free-space threshold (RD-050-15). NULL inherits the global default from the
-- `service.settings` blob, so an existing installation keeps behaving as before.
ALTER TABLE storage_roots ADD COLUMN minimum_free_bytes INTEGER;
