-- Links in a package that point at the same file, so only one of them is downloaded
-- (RD-094-05).
--
-- The key is computed when the links are enqueued rather than derived on the fly: it has to
-- be stable to be shown ("mirror of X") and indexable to be cheap, and a value computed per
-- tick from fuzzy inputs would be neither. NULL means the link belongs to no group, which is
-- the case for every download that predates this and for kinds that are never mirrored.
ALTER TABLE downloads ADD COLUMN mirror_group TEXT;

CREATE INDEX downloads_mirror_idx ON downloads(package_id, mirror_group);
