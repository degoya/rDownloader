-- Manual queue ordering and priorities for packages; LinkGrabber priorities.
ALTER TABLE packages ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
UPDATE packages SET position = rowid;
CREATE INDEX packages_queue_idx ON packages(priority, position);
ALTER TABLE link_candidates ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
