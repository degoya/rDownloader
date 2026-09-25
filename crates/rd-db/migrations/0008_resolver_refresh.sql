ALTER TABLE downloads
ADD COLUMN resolver_refresh_count INTEGER NOT NULL DEFAULT 0
CHECK(resolver_refresh_count BETWEEN 0 AND 1);
