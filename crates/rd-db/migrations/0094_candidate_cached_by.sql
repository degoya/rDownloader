-- RD-130-11: which provider last said it holds a link's file in its own cache.
--
-- The provider's slug (`torbox`, `premiumize`), written together with `cached_at` and cleared
-- together with it, so the two always describe the same check. NULL wherever `cached_at` is
-- NULL, and for the rows RD-120-36 stamped before this column existed: those name no provider,
-- and the interface leaves the name out rather than guessing one.
ALTER TABLE link_candidates ADD COLUMN cached_by TEXT;
