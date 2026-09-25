-- RD-120-36: when a provider last said it holds a link's file in its own cache.
--
-- NULL for every link no check has called cached, which is every row that exists today. A cache
-- is a measurement with an expiry nobody announces, so the time is stored rather than a flag: the
-- interface shows how old the statement is, not just that it was once made.
ALTER TABLE link_candidates ADD COLUMN cached_at TEXT;
