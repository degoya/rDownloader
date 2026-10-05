-- An optional expiry for API and capture tokens (RD-1110-07, audit S13). NULL never expires,
-- which is what every token issued before this column existed keeps. An expired token is
-- refused like a revoked one but stays in the list until it is revoked.
ALTER TABLE capture_tokens ADD COLUMN expires_at TEXT NULL;
